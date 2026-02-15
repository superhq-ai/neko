use async_trait::async_trait;
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::Searcher;
use serde_json::json;
use walkdir::WalkDir;

use super::{schema_object, Tool, ToolContext, ToolResult};
use crate::error::Result;

pub struct MemorySearchTool;

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search across all memory files for matching text. Case-insensitive. Supports regex patterns when regex=true."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        schema_object(
            json!({
                "query": {
                    "type": "string",
                    "description": "Text to search for (case-insensitive). Treated as literal text unless regex=true."
                },
                "max_results": {
                    "type": "integer",
                    "description": "Maximum number of matching lines to return. Default: 20"
                },
                "regex": {
                    "type": "boolean",
                    "description": "Treat query as a regex pattern instead of literal text. Default: false"
                }
            }),
            &["query"],
        )
    }

    async fn execute(&self, params: serde_json::Value, ctx: &ToolContext) -> Result<ToolResult> {
        let query = params["query"].as_str().unwrap_or_default();
        if query.is_empty() {
            return Ok(ToolResult::error("query is required"));
        }

        let use_regex = params["regex"].as_bool().unwrap_or(false);
        let max_results = params["max_results"].as_u64().unwrap_or(20) as usize;
        let memory_dir = ctx.workspace.join("memory");

        if !memory_dir.exists() {
            return Ok(ToolResult::success("No memory directory found. No results."));
        }

        let pattern = if use_regex {
            query.to_string()
        } else {
            regex::escape(query)
        };

        let matcher = match RegexMatcherBuilder::new()
            .case_insensitive(true)
            .build(&pattern)
        {
            Ok(m) => m,
            Err(e) => return Ok(ToolResult::error(format!("Invalid search pattern: {e}"))),
        };

        let mut matches = Vec::new();
        let mut searcher = Searcher::new();

        for entry in WalkDir::new(&memory_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }

            let rel_path = path
                .strip_prefix(&ctx.workspace)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            let _ = searcher.search_path(
                &matcher,
                path,
                UTF8(|line_num, line| {
                    if matches.len() < max_results {
                        matches.push(format!(
                            "{}:{}: {}",
                            rel_path,
                            line_num,
                            line.trim_end()
                        ));
                    }
                    Ok(matches.len() < max_results)
                }),
            );

            if matches.len() >= max_results {
                break;
            }
        }

        if matches.is_empty() {
            Ok(ToolResult::success(format!(
                "No matches found for \"{query}\""
            )))
        } else {
            let count = matches.len();
            let output = matches.join("\n");
            Ok(ToolResult::success(format!(
                "{count} match(es) found:\n{output}"
            )))
        }
    }
}
