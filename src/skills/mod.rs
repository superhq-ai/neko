use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{NekoError, Result};

/// AgentSkills.io-compatible skill.
/// See https://agentskills.io/specification
#[derive(Debug, Clone)]
pub struct Skill {
    // Required fields
    pub name: String,
    pub description: String,
    // Optional fields
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: HashMap<String, String>,
    pub allowed_tools: Vec<String>,
    // Runtime fields
    pub instructions: String,
    pub path: PathBuf,
}

/// YAML frontmatter parsed from SKILL.md
#[derive(Debug, Deserialize)]
struct SkillFrontmatter {
    name: String,
    description: String,
    #[serde(default)]
    license: Option<String>,
    #[serde(default)]
    compatibility: Option<String>,
    #[serde(default)]
    metadata: HashMap<String, String>,
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
}

impl Skill {
    /// Load a skill from a SKILL.md file.
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;

        if !content.starts_with("---") {
            return Err(NekoError::Config(format!(
                "Skill at {} missing YAML frontmatter",
                path.display()
            )));
        }

        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(NekoError::Config(format!(
                "Skill at {} has invalid frontmatter format",
                path.display()
            )));
        }

        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(parts[1]).map_err(|e| {
                NekoError::Config(format!("Failed to parse skill YAML: {e}"))
            })?;

        // Validate name
        validate_skill_name(&frontmatter.name)?;

        let allowed_tools = frontmatter
            .allowed_tools
            .map(|s| s.split_whitespace().map(String::from).collect())
            .unwrap_or_default();

        Ok(Skill {
            name: frontmatter.name,
            description: frontmatter.description,
            license: frontmatter.license,
            compatibility: frontmatter.compatibility,
            metadata: frontmatter.metadata,
            allowed_tools,
            instructions: parts[2].trim().to_string(),
            path: path.parent().unwrap_or(path).to_path_buf(),
        })
    }

    /// Load only metadata (name + description) without the full body.
    /// Used for progressive disclosure.
    pub fn load_metadata(path: &Path) -> Result<(String, String, PathBuf)> {
        let content = std::fs::read_to_string(path)?;

        if !content.starts_with("---") {
            return Err(NekoError::Config(format!(
                "Skill at {} missing YAML frontmatter",
                path.display()
            )));
        }

        let parts: Vec<&str> = content.splitn(3, "---").collect();
        if parts.len() < 3 {
            return Err(NekoError::Config(format!(
                "Skill at {} has invalid frontmatter format",
                path.display()
            )));
        }

        let frontmatter: SkillFrontmatter =
            serde_yaml::from_str(parts[1]).map_err(|e| {
                NekoError::Config(format!("Failed to parse skill YAML: {e}"))
            })?;

        Ok((
            frontmatter.name,
            frontmatter.description,
            path.parent().unwrap_or(path).to_path_buf(),
        ))
    }
}

fn validate_skill_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 {
        return Err(NekoError::Config(
            "Skill name must be 1-64 characters".to_string(),
        ));
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Err(NekoError::Config(
            "Skill name must not start or end with '-'".to_string(),
        ));
    }
    if name.contains("--") {
        return Err(NekoError::Config(
            "Skill name must not contain consecutive hyphens".to_string(),
        ));
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err(NekoError::Config(
            "Skill name may only contain lowercase letters, digits, and hyphens".to_string(),
        ));
    }
    Ok(())
}

/// Discover and load all skills from the workspace skills directory.
pub fn load_skills(workspace: &Path) -> Result<Vec<Skill>> {
    let skills_dir = workspace.join("skills");
    if !skills_dir.exists() {
        return Ok(vec![]);
    }

    let mut skills = Vec::new();

    for entry in walkdir::WalkDir::new(&skills_dir)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();
        if path.file_name().map_or(false, |n| n == "SKILL.md") {
            match Skill::load(path) {
                Ok(skill) => skills.push(skill),
                Err(e) => eprintln!("Warning: Failed to load skill at {}: {e}", path.display()),
            }
        }
    }

    Ok(skills)
}

/// Generate XML for available skills in system prompt (progressive disclosure).
pub fn skills_to_prompt_xml(skills: &[Skill]) -> String {
    if skills.is_empty() {
        return String::new();
    }

    let mut xml = String::from("<available_skills>\n");
    for skill in skills {
        xml.push_str(&format!(
            "  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}/SKILL.md</location>\n  </skill>\n",
            skill.name,
            skill.description,
            skill.path.display(),
        ));
    }
    xml.push_str("</available_skills>");
    xml
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_skill_name() {
        assert!(validate_skill_name("pdf-processing").is_ok());
        assert!(validate_skill_name("data-analysis").is_ok());
        assert!(validate_skill_name("myskill").is_ok());

        assert!(validate_skill_name("").is_err());
        assert!(validate_skill_name("-bad").is_err());
        assert!(validate_skill_name("bad-").is_err());
        assert!(validate_skill_name("bad--name").is_err());
        assert!(validate_skill_name("Bad").is_err());
    }

    #[test]
    fn test_load_skill() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("test-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();

        std::fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: test-skill
description: A test skill for unit testing.
license: MIT
metadata:
  author: test
  version: "1.0"
allowed-tools: Bash Read
---

# Test Skill

Do the test thing.
"#,
        )
        .unwrap();

        let skill = Skill::load(&skill_dir.join("SKILL.md")).unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "A test skill for unit testing.");
        assert_eq!(skill.license.as_deref(), Some("MIT"));
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read"]);
        assert!(skill.instructions.contains("Do the test thing"));
        assert_eq!(skill.metadata.get("author").unwrap(), "test");
    }

    #[test]
    fn test_load_skills_from_workspace() {
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");
        let skill1 = skills_dir.join("my-skill");
        std::fs::create_dir_all(&skill1).unwrap();

        std::fs::write(
            skill1.join("SKILL.md"),
            "---\nname: my-skill\ndescription: Does stuff.\n---\n\nInstructions here.\n",
        )
        .unwrap();

        let skills = load_skills(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
    }

    #[test]
    fn test_skills_to_prompt_xml() {
        let skills = vec![Skill {
            name: "test".to_string(),
            description: "Test skill".to_string(),
            license: None,
            compatibility: None,
            metadata: HashMap::new(),
            allowed_tools: vec![],
            instructions: "Do stuff".to_string(),
            path: PathBuf::from("/skills/test"),
        }];

        let xml = skills_to_prompt_xml(&skills);
        assert!(xml.contains("<available_skills>"));
        assert!(xml.contains("<name>test</name>"));
        assert!(xml.contains("<description>Test skill</description>"));
    }
}
