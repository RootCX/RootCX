use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

pub async fn discover(dirs: &[PathBuf]) -> Vec<SkillEntry> {
    let mut skills = Vec::new();
    for dir in dirs {
        let Ok(mut entries) = tokio::fs::read_dir(dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let skill_md = entry.path().join("SKILL.md");
            if let Ok(true) = tokio::fs::try_exists(&skill_md).await {
                if let Some(skill) = parse_skill_md(&skill_md).await {
                    skills.push(skill);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    skills.retain(|s| seen.insert(s.name.clone()));
    skills
}

async fn parse_skill_md(path: &Path) -> Option<SkillEntry> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    let fm = extract_frontmatter(&content)?;
    let name = extract_field(fm, "name")?;
    let description = extract_field(fm, "description")?;
    Some(SkillEntry { name, description, path: path.to_path_buf() })
}

fn extract_frontmatter(content: &str) -> Option<&str> {
    let rest = content.strip_prefix("---")?;
    let end = rest.find("\n---")?;
    Some(&rest[..end])
}

fn extract_field(frontmatter: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    for line in frontmatter.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix(&prefix) {
            let val = rest.trim().trim_matches('"').trim_matches('\'');
            if !val.is_empty() {
                return Some(val.to_string());
            }
        }
    }
    None
}

pub fn build_catalog(skills: &[SkillEntry]) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let mut out = String::from(concat!(
        "\n\nThe following skills provide specialized instructions for specific tasks.\n",
        "When a task matches a skill's description, use the read tool to load ",
        "the SKILL.md at the listed path before proceeding.\n",
        "When a skill references relative paths, resolve them against the skill's ",
        "directory (the parent of SKILL.md) and use absolute paths in tool calls.\n\n",
        "<available_skills>\n",
    ));
    for s in skills {
        out.push_str(&format!(
            "<skill name=\"{}\" path=\"{}\">{}</skill>\n",
            s.name,
            s.path.display(),
            s.description,
        ));
    }
    out.push_str("</available_skills>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_frontmatter_basic() {
        let content = "---\nname: my-skill\ndescription: Does cool things\n---\n\n# Body";
        let fm = extract_frontmatter(content).unwrap();
        assert_eq!(extract_field(fm, "name").unwrap(), "my-skill");
        assert_eq!(extract_field(fm, "description").unwrap(), "Does cool things");
    }

    #[test]
    fn parse_frontmatter_with_colons_in_value() {
        let content = "---\nname: pdf-tool\ndescription: Use when: user asks about PDFs\n---\n";
        let fm = extract_frontmatter(content).unwrap();
        assert_eq!(
            extract_field(fm, "description").unwrap(),
            "Use when: user asks about PDFs"
        );
    }

    #[test]
    fn parse_frontmatter_quoted_values() {
        let content = "---\nname: \"my-skill\"\ndescription: 'A quoted desc'\n---\n";
        let fm = extract_frontmatter(content).unwrap();
        assert_eq!(extract_field(fm, "name").unwrap(), "my-skill");
        assert_eq!(extract_field(fm, "description").unwrap(), "A quoted desc");
    }

    #[test]
    fn missing_frontmatter_returns_none() {
        assert!(extract_frontmatter("# Just markdown").is_none());
        assert!(extract_frontmatter("---\nname: x\nno closing").is_none());
    }

    #[test]
    fn empty_skills_produces_empty_catalog() {
        assert!(build_catalog(&[]).is_empty());
    }

    #[test]
    fn catalog_includes_all_skills() {
        let skills = vec![
            SkillEntry {
                name: "a".into(),
                description: "Skill A".into(),
                path: PathBuf::from("/skills/a/SKILL.md"),
            },
            SkillEntry {
                name: "b".into(),
                description: "Skill B".into(),
                path: PathBuf::from("/skills/b/SKILL.md"),
            },
        ];
        let cat = build_catalog(&skills);
        assert!(cat.contains("<available_skills>"));
        assert!(cat.contains(r#"<skill name="a""#));
        assert!(cat.contains(r#"<skill name="b""#));
        assert!(cat.contains("</available_skills>"));
    }
}
