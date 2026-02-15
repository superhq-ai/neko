use std::time::Duration;

use async_trait::async_trait;
use monty::{
    CollectStringPrint, LimitedTracker, MontyObject, MontyRun, ResourceLimits, RunProgress,
};
use serde_json::json;

use super::{
    http_request, list_files, read_file, schema_object, write_file, Tool, ToolContext, ToolResult,
};
use crate::config::PythonConfig;
use crate::error::Result;

/// Maximum number of external function calls per execution to prevent infinite loops.
const MAX_EXTERNAL_CALLS: usize = 20;

/// Maximum output size in bytes.
const MAX_OUTPUT_BYTES: usize = 50 * 1024;

/// Bridge holding tool instances that Python can call back into.
struct BridgeTools {
    read_file: read_file::ReadFileTool,
    write_file: write_file::WriteFileTool,
    list_files: list_files::ListFilesTool,
    http_request: http_request::HttpRequestTool,
}

pub struct RunPythonTool {
    config: PythonConfig,
    bridge: BridgeTools,
}

impl RunPythonTool {
    pub fn new(config: PythonConfig, http_allowed_domains: Vec<String>) -> Self {
        Self {
            config,
            bridge: BridgeTools {
                read_file: read_file::ReadFileTool,
                write_file: write_file::WriteFileTool,
                list_files: list_files::ListFilesTool,
                http_request: http_request::HttpRequestTool::new(http_allowed_domains),
            },
        }
    }

    /// Dispatch an external function call to the appropriate bridge tool.
    async fn dispatch_external(
        &self,
        name: &str,
        args: &[MontyObject],
        ctx: &ToolContext,
    ) -> std::result::Result<MontyObject, String> {
        if !self.config.external_functions.contains(&name.to_string()) {
            return Err(format!("Function '{name}' is not in the allowed external functions list"));
        }

        let params = args_to_params(name, args)?;

        let tool: &dyn Tool = match name {
            "read_file" => &self.bridge.read_file,
            "write_file" => &self.bridge.write_file,
            "list_files" => &self.bridge.list_files,
            "http_request" => &self.bridge.http_request,
            _ => return Err(format!("Unknown external function: {name}")),
        };

        let result = tool
            .execute(params, ctx)
            .await
            .map_err(|e| format!("Tool execution error: {e}"))?;

        if result.is_error {
            Err(result.output)
        } else {
            Ok(MontyObject::String(result.output))
        }
    }
}

#[async_trait]
impl Tool for RunPythonTool {
    fn name(&self) -> &str {
        "run_python"
    }

    fn description(&self) -> &str {
        "Execute sandboxed Python code using a minimal interpreter (not CPython). \
         Returns the expression result and any printed output. \
         Use `inputs` to pass named variables into the script scope. \
         Callback functions (if enabled): read_file(path), write_file(path, content), \
         list_files(path), http_request(url, method, body, headers). \
         LIMITATIONS: No imports (no math, json, os, re, or any modules). \
         No with statements, try/except, classes, decorators, generators, or async/await. \
         No str.format() — use f-strings or concatenation instead. \
         Use operators for math: x**0.5 not math.sqrt(x), abs() and round() are builtins. \
         Keep code simple: functions, loops, conditionals, list/dict comprehensions, f-strings."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "code": {
                    "type": "string",
                    "description": "Python source code to execute"
                },
                "inputs": {
                    "type": "object",
                    "description": "Named variables to inject into the script scope (values must be strings, numbers, booleans, or null)"
                }
            }),
            &["code"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let code = match params["code"].as_str() {
            Some(c) => c.to_string(),
            None => return Ok(ToolResult::error("Missing required parameter: code")),
        };

        // Parse input variables
        let (input_names, input_values) = match parse_inputs(&params["inputs"]) {
            Ok(v) => v,
            Err(e) => return Ok(ToolResult::error(format!("Invalid inputs: {e}"))),
        };

        // Collect external function names
        let external_fns: Vec<String> = self.config.external_functions.clone();

        // Compile the Python code
        let runner = match MontyRun::new(
            code,
            "script.py",
            input_names,
            external_fns,
        ) {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::error(format!("Python compilation error: {}", e.summary()))),
        };

        // Set up resource limits
        let limits = ResourceLimits {
            max_allocations: Some(self.config.max_allocations),
            max_duration: Some(Duration::from_secs(self.config.timeout_secs)),
            max_memory: Some(self.config.max_memory),
            gc_interval: None,
            max_recursion_depth: Some(self.config.max_recursion),
        };
        let tracker = LimitedTracker::new(limits);

        // Start execution with iterative model (to handle external function calls)
        let mut printer = CollectStringPrint::new();

        let mut progress = match tokio::task::spawn_blocking({
            let runner = runner.clone();
            let input_values = input_values.clone();
            let mut printer_inner = CollectStringPrint::new();
            move || {
                let result = runner.start(input_values, tracker, &mut printer_inner);
                (result, printer_inner)
            }
        })
        .await
        {
            Ok((Ok(p), p_inner)) => {
                // Merge printed output
                let printed = p_inner.into_output();
                if !printed.is_empty() {
                    // Replay into our main printer by pushing chars
                    for ch in printed.chars() {
                        let _ = monty::PrintWriter::stdout_push(&mut printer, ch);
                    }
                }
                p
            }
            Ok((Err(e), _)) => {
                return Ok(ToolResult::error(format!("Python error: {}", e.summary())));
            }
            Err(e) => {
                return Ok(ToolResult::error(format!("Execution panicked: {e}")));
            }
        };

        // Process the run loop — handle external function calls
        let mut call_count = 0usize;

        loop {
            match progress {
                RunProgress::Complete(obj) => {
                    let printed = printer.into_output();
                    let output = format_output(&obj, &printed);
                    return Ok(ToolResult::success(output));
                }

                RunProgress::FunctionCall {
                    function_name,
                    args,
                    state,
                    ..
                } => {
                    call_count += 1;
                    if call_count > MAX_EXTERNAL_CALLS {
                        return Ok(ToolResult::error(format!(
                            "Exceeded maximum of {MAX_EXTERNAL_CALLS} external function calls"
                        )));
                    }

                    // Dispatch the external function call (async)
                    let ext_result = self
                        .dispatch_external(&function_name, &args, ctx)
                        .await;

                    // Resume execution with the result
                    let resume_value = match ext_result {
                        Ok(obj) => obj,
                        Err(e) => MontyObject::String(format!("Error: {e}")),
                    };

                    match tokio::task::spawn_blocking({
                        let mut printer_inner = CollectStringPrint::new();
                        move || {
                            let result = state.run(resume_value, &mut printer_inner);
                            (result, printer_inner)
                        }
                    })
                    .await
                    {
                        Ok((Ok(p), p_inner)) => {
                            let printed = p_inner.into_output();
                            if !printed.is_empty() {
                                for ch in printed.chars() {
                                    let _ = monty::PrintWriter::stdout_push(&mut printer, ch);
                                }
                            }
                            progress = p;
                        }
                        Ok((Err(e), _)) => {
                            return Ok(ToolResult::error(format!(
                                "Python error: {}",
                                e.summary()
                            )));
                        }
                        Err(e) => {
                            return Ok(ToolResult::error(format!("Execution panicked: {e}")));
                        }
                    }
                }

                RunProgress::OsCall { state, .. } => {
                    // OS calls are not allowed — return an error to the sandbox
                    let err_obj = MontyObject::String(
                        "Error: OS operations are not permitted in the sandbox".to_string(),
                    );

                    match tokio::task::spawn_blocking({
                        let mut printer_inner = CollectStringPrint::new();
                        move || {
                            let result = state.run(err_obj, &mut printer_inner);
                            (result, printer_inner)
                        }
                    })
                    .await
                    {
                        Ok((Ok(p), p_inner)) => {
                            let printed = p_inner.into_output();
                            if !printed.is_empty() {
                                for ch in printed.chars() {
                                    let _ = monty::PrintWriter::stdout_push(&mut printer, ch);
                                }
                            }
                            progress = p;
                        }
                        Ok((Err(e), _)) => {
                            return Ok(ToolResult::error(format!(
                                "Python error: {}",
                                e.summary()
                            )));
                        }
                        Err(e) => {
                            return Ok(ToolResult::error(format!("Execution panicked: {e}")));
                        }
                    }
                }

                RunProgress::ResolveFutures(_) => {
                    return Ok(ToolResult::error(
                        "Async/await is not supported in the sandbox",
                    ));
                }
            }
        }
    }
}

/// Parse the `inputs` JSON object into (names, values) for MontyRun.
fn parse_inputs(
    inputs: &serde_json::Value,
) -> std::result::Result<(Vec<String>, Vec<MontyObject>), String> {
    let obj = match inputs {
        serde_json::Value::Object(map) => map,
        serde_json::Value::Null => return Ok((vec![], vec![])),
        _ => return Err("inputs must be an object".to_string()),
    };

    let mut names = Vec::with_capacity(obj.len());
    let mut values = Vec::with_capacity(obj.len());

    for (key, val) in obj {
        names.push(key.clone());
        values.push(json_to_monty(val)?);
    }

    Ok((names, values))
}

/// Convert a JSON value to a MontyObject.
fn json_to_monty(val: &serde_json::Value) -> std::result::Result<MontyObject, String> {
    match val {
        serde_json::Value::Null => Ok(MontyObject::None),
        serde_json::Value::Bool(b) => Ok(MontyObject::Bool(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(MontyObject::Int(i))
            } else if let Some(f) = n.as_f64() {
                Ok(MontyObject::Float(f))
            } else {
                Err(format!("Unsupported number: {n}"))
            }
        }
        serde_json::Value::String(s) => Ok(MontyObject::String(s.clone())),
        serde_json::Value::Array(arr) => {
            let items: std::result::Result<Vec<MontyObject>, String> =
                arr.iter().map(json_to_monty).collect();
            Ok(MontyObject::List(items?))
        }
        serde_json::Value::Object(map) => {
            let pairs: std::result::Result<Vec<(MontyObject, MontyObject)>, String> = map
                .iter()
                .map(|(k, v)| {
                    Ok((MontyObject::String(k.clone()), json_to_monty(v)?))
                })
                .collect();
            Ok(MontyObject::Dict(pairs?.into()))
        }
    }
}

/// Convert MontyObject args from Python function calls into JSON params for a bridge tool.
fn args_to_params(
    fn_name: &str,
    args: &[MontyObject],
) -> std::result::Result<serde_json::Value, String> {
    match fn_name {
        "read_file" => {
            let path = get_string_arg(args, 0, "path")?;
            Ok(json!({ "path": path }))
        }
        "write_file" => {
            let path = get_string_arg(args, 0, "path")?;
            let content = get_string_arg(args, 1, "content")?;
            Ok(json!({ "path": path, "content": content }))
        }
        "list_files" => {
            let path = if args.is_empty() {
                ".".to_string()
            } else {
                get_string_arg(args, 0, "path")?
            };
            Ok(json!({ "path": path }))
        }
        "http_request" => {
            let url = get_string_arg(args, 0, "url")?;
            let mut params = json!({ "url": url });
            if args.len() > 1 {
                if let Ok(method) = get_string_arg(args, 1, "method") {
                    params["method"] = json!(method);
                }
            }
            if args.len() > 2 {
                if let Ok(body) = get_string_arg(args, 2, "body") {
                    params["body"] = json!(body);
                }
            }
            Ok(params)
        }
        _ => Err(format!("Unknown function: {fn_name}")),
    }
}

/// Extract a string argument from a MontyObject slice.
fn get_string_arg(
    args: &[MontyObject],
    index: usize,
    name: &str,
) -> std::result::Result<String, String> {
    match args.get(index) {
        Some(MontyObject::String(s)) => Ok(s.clone()),
        Some(other) => Err(format!(
            "Expected string for argument '{name}', got: {other:?}"
        )),
        None => Err(format!("Missing required argument: {name}")),
    }
}

/// Format the final output combining printed text and expression result.
fn format_output(result: &MontyObject, printed: &str) -> String {
    let mut output = String::new();

    if !printed.is_empty() {
        output.push_str(printed);
        if !printed.ends_with('\n') {
            output.push('\n');
        }
    }

    // Append expression result (REPL-style) unless it's None
    if !matches!(result, MontyObject::None) {
        let display = monty_obj_to_display(result);
        output.push_str(&format!("=> {display}"));
    }

    // Truncate if too large
    if output.len() > MAX_OUTPUT_BYTES {
        output.truncate(MAX_OUTPUT_BYTES);
        output.push_str("\n... [output truncated]");
    }

    if output.is_empty() {
        "(no output)".to_string()
    } else {
        output
    }
}

/// Convert a MontyObject to a display string.
fn monty_obj_to_display(obj: &MontyObject) -> String {
    match obj {
        MontyObject::None => "None".to_string(),
        MontyObject::Bool(b) => if *b { "True" } else { "False" }.to_string(),
        MontyObject::Int(i) => i.to_string(),
        MontyObject::Float(f) => format!("{f}"),
        MontyObject::String(s) => format!("'{s}'"),
        MontyObject::List(items) => {
            let inner: Vec<String> = items.iter().map(monty_obj_to_display).collect();
            format!("[{}]", inner.join(", "))
        }
        MontyObject::Tuple(items) => {
            let inner: Vec<String> = items.iter().map(monty_obj_to_display).collect();
            if items.len() == 1 {
                format!("({},)", inner[0])
            } else {
                format!("({})", inner.join(", "))
            }
        }
        MontyObject::Dict(pairs) => {
            let inner: Vec<String> = pairs
                .into_iter()
                .map(|(k, v)| {
                    format!("{}: {}", monty_obj_to_display(k), monty_obj_to_display(v))
                })
                .collect();
            format!("{{{}}}", inner.join(", "))
        }
        MontyObject::Ellipsis => "...".to_string(),
        MontyObject::Bytes(b) => format!("b'{}'", String::from_utf8_lossy(b)),
        MontyObject::Set(items) => {
            let inner: Vec<String> = items.iter().map(monty_obj_to_display).collect();
            format!("{{{}}}", inner.join(", "))
        }
        _ => format!("{obj:?}"),
    }
}
