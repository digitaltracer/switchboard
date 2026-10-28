//! The skills and commands an agent can run, for the phone's `/` picker: names and one-line
//! descriptions only. Claude Code's come from `<claude dir>/skills/*/SKILL.md`, `<claude
//! dir>/commands/**/*.md`, the same two folders under the agent's repo's `.claude/`, and
//! installed plugins; Codex's from `<codex home>/skills/*/SKILL.md` and `<codex home>/prompts/*.md`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::proto::SkillInfo;

const MAX_SKILLS: usize = 300;
const MAX_DESCRIPTION: usize = 160;
/// Only a file's start is read; the frontmatter and first lines are all that's needed.
const HEAD_BYTES: u64 = 8 * 1024;

pub fn claude(claude_dir: &Path, repo: Option<&Path>) -> Vec<SkillInfo> {
    let mut out = Vec::new();
    if let Some(repo) = repo {
        skill_dirs(&repo.join(".claude/skills"), "/", "", "repo", &mut out);
        commands(&repo.join(".claude/commands"), "/", "repo", &mut out);
    }
    skill_dirs(&claude_dir.join("skills"), "/", "", "user", &mut out);
    commands(&claude_dir.join("commands"), "/", "user", &mut out);
    for (plugin, path) in plugins(claude_dir) {
        let prefix = format!("{plugin}:");
        skill_dirs(&path.join("skills"), "/", &prefix, "plugin", &mut out);
        let mut cmds = Vec::new();
        commands(&path.join("commands"), "/", "plugin", &mut cmds);
        out.extend(cmds.into_iter().map(|mut s| {
            s.name = format!("/{prefix}{}", &s.name[1..]);
            s
        }));
    }
    builtin(
        &mut out,
        &[
            ("/clear", "Start a new conversation"),
            ("/compact", "Summarise the conversation to free up context"),
        ],
    );
    finish(out)
}

pub fn codex(codex_home: &Path) -> Vec<SkillInfo> {
    let mut out = Vec::new();
    // Codex runs a skill named in the message with `$`.
    skill_dirs(&codex_home.join("skills"), "$", "", "user", &mut out);
    let mut prompts = Vec::new();
    commands(&codex_home.join("prompts"), "/", "user", &mut prompts);
    out.extend(prompts.into_iter().map(|mut s| {
        s.name = format!("/prompts:{}", &s.name[1..]);
        s
    }));
    builtin(
        &mut out,
        &[
            ("/new", "Start a new conversation"),
            ("/compact", "Summarise the conversation to free up context"),
        ],
    );
    finish(out)
}

fn builtin(out: &mut Vec<SkillInfo>, list: &[(&str, &str)]) {
    out.extend(list.iter().map(|(name, description)| SkillInfo {
        name: (*name).into(),
        description: (*description).into(),
        source: "builtin".into(),
    }));
}

/// Unique by name, the first found winning: the repo's over yours over a plugin's.
fn finish(out: Vec<SkillInfo>) -> Vec<SkillInfo> {
    let mut seen = std::collections::HashSet::new();
    out.into_iter()
        .filter(|s| seen.insert(s.name.clone()))
        .take(MAX_SKILLS)
        .collect()
}

/// `<dir>/<skill>/SKILL.md` for each skill folder, named by its frontmatter `name`.
fn skill_dirs(dir: &Path, sigil: &str, prefix: &str, source: &str, out: &mut Vec<SkillInfo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut found: Vec<_> = entries
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let text = head(&e.path().join("SKILL.md"))?;
            let (meta, body) = frontmatter(&text);
            if meta.get("user-invocable").map(String::as_str) == Some("false") {
                return None;
            }
            let name = meta
                .get("name")
                .cloned()
                .unwrap_or_else(|| e.file_name().to_string_lossy().into_owned());
            Some(SkillInfo {
                name: format!("{sigil}{prefix}{name}"),
                description: describe(&meta, body),
                source: source.into(),
            })
        })
        .collect();
    found.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(found);
}

/// `<dir>/**/*.md`, a file in a subfolder named `sub:file`.
fn commands(dir: &Path, sigil: &str, source: &str, out: &mut Vec<SkillInfo>) {
    let mut found = Vec::new();
    walk(dir, "", 0, &mut |name, path| {
        if let Some(text) = head(path) {
            let (meta, body) = frontmatter(&text);
            found.push(SkillInfo {
                name: format!("{sigil}{name}"),
                description: describe(&meta, body),
                source: source.into(),
            });
        }
    });
    found.sort_by(|a, b| a.name.cmp(&b.name));
    out.extend(found);
}

fn walk(dir: &Path, prefix: &str, depth: usize, f: &mut dyn FnMut(&str, &Path)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let file = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if file.starts_with('.') {
            continue;
        }
        if path.is_dir() && depth < 2 {
            walk(&path, &format!("{prefix}{file}:"), depth + 1, f);
        } else if let Some(stem) = file.strip_suffix(".md") {
            f(&format!("{prefix}{stem}"), &path);
        }
    }
}

/// Installed plugins and where each lives, from `plugins/installed_plugins.json`.
fn plugins(claude_dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let Some(text) =
        std::fs::read_to_string(claude_dir.join("plugins/installed_plugins.json")).ok()
    else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(map) = v.get("plugins").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut out: Vec<_> = map
        .iter()
        .filter_map(|(key, installs)| {
            let path = installs.as_array()?.first()?.get("installPath")?.as_str()?;
            let name = key.split('@').next().unwrap_or(key);
            Some((name.to_owned(), path.into()))
        })
        .collect();
    out.sort();
    out
}

fn head(path: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

/// Top-level `key: value` lines of a `---` frontmatter block, folded values joined, and the rest.
fn frontmatter(text: &str) -> (HashMap<String, String>, &str) {
    let mut meta = HashMap::new();
    let Some(rest) = text.strip_prefix("---\n") else {
        return (meta, text);
    };
    let Some(end) = rest.find("\n---") else {
        return (meta, text);
    };
    let body = rest[end + 4..].trim_start_matches(['-', '\n']);
    let mut current: Option<String> = None;
    for line in rest[..end].lines() {
        if line.starts_with([' ', '\t']) {
            if let Some(key) = &current {
                let v: &mut String = meta.entry(key.clone()).or_default();
                if !v.is_empty() {
                    v.push(' ');
                }
                v.push_str(line.trim());
            }
            continue;
        }
        current = None;
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim().to_owned(), v.trim());
        match v {
            // A block value continues on the indented lines below.
            ">" | "|" | ">-" | "|-" => {
                meta.insert(k.clone(), String::new());
                current = Some(k);
            }
            // A nested map, like `metadata:`; its keys aren't top-level.
            "" => {}
            _ => {
                meta.insert(k, v.trim_matches(['"', '\'']).to_owned());
            }
        }
    }
    (meta, body)
}

/// The frontmatter's description, else the first line of prose.
fn describe(meta: &HashMap<String, String>, body: &str) -> String {
    let text = meta
        .get("description")
        .filter(|d| !d.is_empty())
        .cloned()
        .or_else(|| {
            body.lines()
                .map(str::trim)
                .find(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("```"))
                .map(str::to_owned)
        })
        .unwrap_or_default();
    clip(&text, MAX_DESCRIPTION)
}

fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
#[path = "skills_tests.rs"]
mod tests;
