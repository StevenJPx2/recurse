//! `recurse skills`: local discovery over the same search path the daemon uses.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use recurse_daemon::DaemonOptions;
use recurse_daemon::skills::discover;
use recurse_protocol::SkillInfo;

use crate::args::Args;

pub fn run(args: &Args) -> Result<ExitCode, String> {
    let options = DaemonOptions::from_env()?;
    let cwd = std::env::current_dir().ok();
    let dirs = options.skill_dirs(cwd.as_deref());
    let skills = discover(&dirs);
    let (command, names) = args
        .positional
        .split_first()
        .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));

    match (command, names) {
        ("list", []) => {
            for skill in skills.iter().filter(|skill| !skill.hidden) {
                println!("{}\t{}", skill.name, skill.description);
            }

            Ok(ExitCode::SUCCESS)
        }
        ("get", names) if !names.is_empty() => {
            let documents = names
                .iter()
                .map(|name| document(find(&skills, name)?, args.has("--full")))
                .collect::<Result<Vec<_>, _>>()?;

            println!("{}", documents.join("\n\n"));
            Ok(ExitCode::SUCCESS)
        }
        ("path", []) => {
            for dir in &dirs {
                println!("{}", dir.display());
            }

            Ok(ExitCode::SUCCESS)
        }
        ("path", [name]) => {
            println!("{}", skill_dir(find(&skills, name)?).display());
            Ok(ExitCode::SUCCESS)
        }
        _ => Err("usage: recurse skills list | get <name>… [--full] | path [name]".into()),
    }
}

fn find<'a>(skills: &'a [SkillInfo], name: &str) -> Result<&'a SkillInfo, String> {
    skills
        .iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| format!("no skill named {name}"))
}

fn skill_dir(skill: &SkillInfo) -> PathBuf {
    Path::new(&skill.path)
        .parent()
        .map_or_else(|| PathBuf::from(&skill.path), Path::to_path_buf)
}

/// `# name` + SKILL.md, and with `full` every `references/*` file under its own heading.
fn document(skill: &SkillInfo, full: bool) -> Result<String, String> {
    let body = std::fs::read_to_string(&skill.path)
        .map_err(|error| format!("read {}: {error}", skill.path))?;
    let mut text = format!("# {}\n\n{}", skill.name, body.trim_end());

    if !full {
        return Ok(text);
    }

    let references = skill_dir(skill).join("references");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&references)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.is_file())
                .collect()
        })
        .unwrap_or_default();

    files.sort();

    for file in files {
        let content = std::fs::read_to_string(&file)
            .map_err(|error| format!("read {}: {error}", file.display()))?;
        let name = file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();

        text.push_str(&format!(
            "\n\n## references/{name}\n\n{}",
            content.trim_end()
        ));
    }

    Ok(text)
}
