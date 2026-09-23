//! Skill discovery (§5): directories with a `SKILL.md` carrying YAML-ish frontmatter.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use recurse_protocol::{SkillInfo, bounds, is_valid_name};

/// Search path, later entries shadowing earlier ones: built-in, user config, then project.
pub fn search_dirs(builtin: Option<&Path>, config_dir: &Path, cwd: Option<&Path>) -> Vec<PathBuf> {
    builtin
        .map(Path::to_path_buf)
        .into_iter()
        .chain(std::iter::once(config_dir.join("skills")))
        .chain(cwd.map(|cwd| cwd.join(".recurse/skills")))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// Every valid skill under `dirs`, sorted by name, later directories winning.
pub fn discover(dirs: &[PathBuf]) -> Vec<SkillInfo> {
    let mut skills = BTreeMap::new();

    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();

        paths.sort();

        for skill in paths.iter().filter_map(|path| load(path)) {
            skills.insert(skill.name.clone(), skill);
        }
    }

    skills.into_values().collect()
}

fn load(dir: &Path) -> Option<SkillInfo> {
    let path = dir.join("SKILL.md");
    let text = std::fs::read_to_string(&path).ok()?;
    let fields = frontmatter(&text)?;
    let fallback = dir.file_name()?.to_string_lossy().into_owned();
    let name = fields.get("name").cloned().unwrap_or(fallback);

    if !is_valid_name(&name) {
        return None;
    }

    let description: String = fields
        .get("description")
        .map(|text| text.chars().take(bounds::SKILL_DESCRIPTION).collect())
        .unwrap_or_default();

    Some(SkillInfo {
        name,
        description,
        path: path.to_string_lossy().into_owned(),
        python: fields
            .get("python")
            .filter(|value| !value.is_empty())
            .cloned(),
        hidden: fields.get("hidden").is_some_and(|value| value == "true"),
    })
}

/// Parses `---`-delimited `key: value` lines. Indented lines continue the previous value, and
/// `>` / `|` block markers are accepted as the start of such a continuation.
pub fn frontmatter(text: &str) -> Option<BTreeMap<String, String>> {
    let mut lines = text.lines();

    if lines.next()?.trim_end() != "---" {
        return None;
    }

    let mut fields = BTreeMap::new();
    let mut last: Option<String> = None;

    for line in lines {
        if line.trim_end() == "---" {
            return Some(fields);
        }

        if line.starts_with([' ', '\t']) {
            if let Some(value) = last.as_ref().and_then(|key| fields.get_mut(key)) {
                append(value, line.trim());
            }

            continue;
        }

        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().to_owned();
        let value = unquote(value.trim());
        let value = if matches!(value, ">" | "|" | ">-" | "|-") {
            ""
        } else {
            value
        };

        fields.insert(key.clone(), value.to_owned());
        last = Some(key);
    }

    None
}

fn append(value: &mut String, more: &str) {
    if !value.is_empty() && !more.is_empty() {
        value.push(' ');
    }

    value.push_str(more);
}

fn unquote(value: &str) -> &str {
    ['"', '\'']
        .iter()
        .find_map(|quote| value.strip_prefix(*quote)?.strip_suffix(*quote))
        .unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::{discover, frontmatter};

    #[test]
    fn parses_frontmatter_fields() {
        let text = "---\nname: release-audit\ndescription: >\n  Audit a release\n  candidate.\npython: \"release_audit:main\"\nhidden: false\n---\n# body\n";
        let fields = frontmatter(text).unwrap();

        assert_eq!(fields["name"], "release-audit");
        assert_eq!(fields["description"], "Audit a release candidate.");
        assert_eq!(fields["python"], "release_audit:main");
        assert_eq!(fields["hidden"], "false");
        assert!(frontmatter("no frontmatter").is_none());
        assert!(frontmatter("---\nname: x\n").is_none());
    }

    #[test]
    fn later_directories_shadow_earlier_ones() {
        let root = std::env::temp_dir().join(format!("recurse-skills-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        for (dir, description, hidden) in [("a", "first", "false"), ("b", "second", "true")] {
            let skill = root.join(dir).join("core");
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: core\ndescription: {description}\nhidden: {hidden}\n---\n"),
            )
            .unwrap();
        }

        std::fs::create_dir_all(root.join("a/Bad_Name")).unwrap();
        std::fs::write(
            root.join("a/Bad_Name/SKILL.md"),
            "---\ndescription: x\n---\n",
        )
        .unwrap();

        let skills = discover(&[root.join("a"), root.join("b")]);

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].description, "second");
        assert!(skills[0].hidden);
        assert_eq!(skills[0].python, None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
