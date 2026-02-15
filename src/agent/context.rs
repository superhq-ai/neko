use std::path::Path;

use walkdir::WalkDir;

use crate::config::AgentConfig;
use crate::skills::{self, Skill};

const MAX_CORE_MEMORY_CHARS: usize = 2000;

const DEFAULT_INSTRUCTIONS: &str = "\
You are Neko, a helpful AI assistant with persistent memory.

## Memory System
Your memory is file-based in the `memory/` directory:
- **MEMORY.md** (core memory): Always loaded below. Keep concise (\u{2264}2000 chars). Store key facts and user preferences.
- **Daily logs** (YYYY-MM-DD.md): Today's and yesterday's logs loaded below. Use for session notes.
- **Recall** (recall/*.md): Past conversations, auto-logged. Search with `memory_search`.

### Memory Tools
- `memory_write(file, content, append)` \u{2014} Write/append to a memory file
- `memory_replace(file, old_text, new_text)` \u{2014} Update or delete facts (empty new_text = delete)
- `memory_search(query)` \u{2014} Search across all memory files

### Guidelines
- Update MEMORY.md when you learn important facts about the user
- Use `memory_replace` to correct outdated info \u{2014} don't let stale facts accumulate
- Use daily logs for ephemeral notes, MEMORY.md for durable facts
- Search recall logs when you need context from past conversations

Be concise and helpful.";

/// Build the memory file tree listing with char counts.
fn build_memory_file_tree(workspace: &Path) -> Option<String> {
    let memory_dir = workspace.join("memory");
    if !memory_dir.exists() {
        return None;
    }

    let mut entries: Vec<(String, usize)> = Vec::new();

    for entry in WalkDir::new(&memory_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let rel_path = path
            .strip_prefix(workspace)
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();

        let chars = std::fs::read_to_string(path)
            .map(|c| c.len())
            .unwrap_or(0);

        entries.push((rel_path, chars));
    }

    if entries.is_empty() {
        return None;
    }

    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut lines = vec!["## Memory Files".to_string()];
    for (path, chars) in &entries {
        lines.push(format!("  {path} ({chars} chars)"));
    }

    Some(lines.join("\n"))
}

/// Build the system instructions for the agent.
pub fn build_instructions(config: &AgentConfig, workspace: &Path, skills: &[Skill]) -> String {
    let mut parts = Vec::new();

    // Base instructions
    if let Some(instructions) = &config.instructions {
        parts.push(instructions.clone());
    } else {
        parts.push(DEFAULT_INSTRUCTIONS.to_string());
    }

    // Memory file tree
    if let Some(tree) = build_memory_file_tree(workspace) {
        parts.push(format!("\n{tree}"));
    }

    // Load MEMORY.md
    let memory_dir = workspace.join("memory");
    let memory_md_path = memory_dir.join("MEMORY.md");
    if let Ok(content) = std::fs::read_to_string(&memory_md_path) {
        let mut section = String::from("\n## Persistent Memory\n");

        // Size constraint warning
        if content.len() > MAX_CORE_MEMORY_CHARS {
            section.push_str(&format!(
                "\n\u{26a0} MEMORY.md is {}/{} chars. Compact it \u{2014} move less-critical info to other files or delete stale entries with memory_replace.\n",
                content.len(),
                MAX_CORE_MEMORY_CHARS
            ));
        }

        section.push('\n');
        section.push_str(&content);
        parts.push(section);
    }

    // Load today's daily log
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let daily_log_path = memory_dir.join(format!("{today}.md"));

    // Ensure daily log exists
    if !daily_log_path.exists() {
        let _ = std::fs::create_dir_all(&memory_dir);
        let _ = std::fs::write(&daily_log_path, format!("# Daily Log: {today}\n\n"));
    }

    if let Ok(daily_log) = std::fs::read_to_string(&daily_log_path) {
        if daily_log.lines().count() > 2 {
            parts.push(format!("\n## Today's Log\n\n{daily_log}"));
        }
    }

    // Load also yesterday's log (OpenClaw pattern)
    let yesterday = (chrono::Local::now() - chrono::Duration::days(1))
        .format("%Y-%m-%d")
        .to_string();
    let yesterday_log_path = memory_dir.join(format!("{yesterday}.md"));
    if let Ok(yesterday_log) = std::fs::read_to_string(&yesterday_log_path) {
        if yesterday_log.lines().count() > 2 {
            parts.push(format!("\n## Yesterday's Log\n\n{yesterday_log}"));
        }
    }

    // Available skills (progressive disclosure — just metadata)
    let xml = skills::skills_to_prompt_xml(skills);
    if !xml.is_empty() {
        parts.push(format!(
            "\n## Available Skills\n\n\
             The following skills are available. To activate a skill, read its SKILL.md file.\n\n\
             {xml}"
        ));
    }

    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_build_instructions_default() {
        let tmp = TempDir::new().unwrap();
        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &[]);
        assert!(instructions.contains("Neko"));
        assert!(instructions.contains("memory_search"));
        assert!(instructions.contains("memory_replace"));
    }

    #[test]
    fn test_build_instructions_with_memory() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "# Important\n\nRemember this.").unwrap();

        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &[]);
        assert!(instructions.contains("Remember this"));
        assert!(instructions.contains("Persistent Memory"));
    }

    #[test]
    fn test_build_instructions_with_skills() {
        let tmp = TempDir::new().unwrap();
        let skills = vec![Skill {
            name: "test-skill".to_string(),
            description: "A test skill".to_string(),
            license: None,
            compatibility: None,
            metadata: std::collections::HashMap::new(),
            allowed_tools: vec![],
            instructions: "Do stuff".to_string(),
            path: std::path::PathBuf::from("/skills/test-skill"),
        }];

        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &skills);
        assert!(instructions.contains("available_skills"));
        assert!(instructions.contains("test-skill"));
    }

    #[test]
    fn test_memory_file_tree_appears() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "hello").unwrap();

        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &[]);
        assert!(instructions.contains("## Memory Files"));
        assert!(instructions.contains("memory/MEMORY.md"));
        assert!(instructions.contains("chars)"));
    }

    #[test]
    fn test_memory_size_warning() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();

        // Create content exceeding 2000 chars
        let big_content = "x".repeat(2500);
        std::fs::write(mem_dir.join("MEMORY.md"), &big_content).unwrap();

        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &[]);
        assert!(instructions.contains("\u{26a0}"));
        assert!(instructions.contains("2500/2000"));
        assert!(instructions.contains("Compact it"));
    }

    #[test]
    fn test_no_size_warning_under_limit() {
        let tmp = TempDir::new().unwrap();
        let mem_dir = tmp.path().join("memory");
        std::fs::create_dir_all(&mem_dir).unwrap();
        std::fs::write(mem_dir.join("MEMORY.md"), "short").unwrap();

        let config = AgentConfig::default();
        let instructions = build_instructions(&config, tmp.path(), &[]);
        assert!(!instructions.contains("Compact it"));
    }
}
