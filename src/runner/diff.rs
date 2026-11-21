//! A read-only unified diff of one file a session changed, since the commit it started from,
//! a page of hunks at a time. Only a file the session's own change list names can be read, so a
//! path can't reach outside the worktree. Binary files give their size only.

use std::path::{Component, Path};

use serde_json::{Value, json};

/// Unchanged lines kept around each change; the phone folds long runs of them.
pub const CONTEXT: u32 = 25;
/// Lines per page; a page always holds at least one hunk, cut short if it alone is longer.
pub const PAGE_LINES: usize = 600;
pub const MAX_LINE_CHARS: usize = 2000;

pub async fn diff(worktree: &Path, base: &str, path: &str, from: u32) -> Result<Value, String> {
    if !plain_relative(path) {
        return Err("not a changed file".into());
    }
    let tracked = lines(
        &git(
            worktree,
            &["diff", "--name-only", "--no-renames", base, "--"],
        )
        .await?,
    );
    let untracked = !tracked.iter().any(|p| p == path)
        && lines(&git(worktree, &["ls-files", "--others", "--exclude-standard"]).await?)
            .iter()
            .any(|p| p == path);
    if !untracked && !tracked.iter().any(|p| p == path) {
        return Err("not a changed file".into());
    }
    let file = worktree.join(path);
    let (binary, added, deleted) = if untracked {
        (looks_binary(&file), None, None)
    } else {
        let stat = git(
            worktree,
            &["diff", "--numstat", "--no-renames", base, "--", path],
        )
        .await?;
        let mut f = stat.split('\t');
        let (a, d) = (f.next().unwrap_or("-"), f.next().unwrap_or("-"));
        (a == "-", a.parse::<u64>().ok(), d.parse::<u64>().ok())
    };
    if binary {
        let size = match std::fs::symlink_metadata(&file) {
            Ok(m) => Some(m.len()),
            Err(_) => git(worktree, &["cat-file", "-s", &format!("{base}:{path}")])
                .await
                .ok()
                .and_then(|s| s.trim().parse().ok()),
        };
        return Ok(
            json!({ "path": path, "binary": true, "size": size, "hunk_count": 0, "hunks": [], "next": null }),
        );
    }
    let context = format!("-U{CONTEXT}");
    let text = if untracked {
        git_any(
            worktree,
            &[
                "diff",
                "--no-index",
                "--no-color",
                &context,
                "--",
                "/dev/null",
                path,
            ],
        )
        .await?
    } else {
        git(
            worktree,
            &[
                "diff",
                "--no-renames",
                "--no-color",
                &context,
                base,
                "--",
                path,
            ],
        )
        .await?
    };
    let hunks = parse(&text);
    let count = hunks.len() as u32;
    let mut page = Vec::new();
    let mut total = 0;
    let mut next = None;
    for (i, mut h) in hunks.into_iter().enumerate().skip(from as usize) {
        if !page.is_empty() && total + h.lines.len() > PAGE_LINES {
            next = Some(i as u32);
            break;
        }
        if h.lines.len() > PAGE_LINES {
            h.lines.truncate(PAGE_LINES);
            h.cut = true;
        }
        total += h.lines.len();
        page.push(h.to_json());
    }
    Ok(json!({
        "path": path, "binary": false, "size": null, "added": added, "deleted": deleted,
        "hunk_count": count, "hunks": page, "next": next,
    }))
}

/// A relative path with no `..`, no root and no NUL: the shape git prints changed files in.
fn plain_relative(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\0')
        && Path::new(p)
            .components()
            .all(|c| matches!(c, Component::Normal(_)))
}

fn lines(s: &str) -> Vec<String> {
    s.lines().map(str::to_owned).collect()
}

/// Git's own test: a NUL in the first 8000 bytes. A symlink is its target's name, so text.
pub fn looks_binary(file: &Path) -> bool {
    use std::io::Read;
    if std::fs::symlink_metadata(file).is_ok_and(|m| m.file_type().is_symlink()) {
        return false;
    }
    let mut buf = [0u8; 8000];
    std::fs::File::open(file)
        .and_then(|mut f| f.read(&mut buf))
        .is_ok_and(|n| buf[..n].contains(&0))
}

#[derive(Debug, PartialEq)]
pub struct Hunk {
    pub header: String,
    pub old_start: u32,
    pub new_start: u32,
    /// (`' '`, `'+'` or `'-'`, old line number, new line number, text)
    pub lines: Vec<(char, Option<u32>, Option<u32>, String)>,
    pub cut: bool,
}

impl Hunk {
    fn to_json(&self) -> Value {
        json!({
            "header": self.header,
            "old_start": self.old_start,
            "new_start": self.new_start,
            "lines": self.lines.iter().map(|(k, o, n, t)| json!({ "kind": k.to_string(), "old": o, "new": n, "text": t })).collect::<Vec<_>>(),
            "cut": self.cut,
        })
    }
}

/// The hunks of a unified diff, numbered as each side counts them.
pub fn parse(text: &str) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let (mut old, mut new) = (0u32, 0u32);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("@@ ") {
            let Some((ranges, _)) = rest.split_once(" @@") else {
                continue;
            };
            let mut r = ranges.split(' ');
            let start = |s: Option<&str>, sign: char| {
                s.and_then(|s| s.strip_prefix(sign))
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0)
            };
            old = start(r.next(), '-');
            new = start(r.next(), '+');
            out.push(Hunk {
                header: line.chars().take(MAX_LINE_CHARS).collect(),
                old_start: old,
                new_start: new,
                lines: Vec::new(),
                cut: false,
            });
            continue;
        }
        let Some(h) = out.last_mut() else {
            continue;
        };
        let mut chars = line.chars();
        let kind = chars.next();
        let body: String = chars.take(MAX_LINE_CHARS).collect();
        match kind {
            Some(' ') => {
                h.lines.push((' ', Some(old), Some(new), body));
                old += 1;
                new += 1;
            }
            Some('-') => {
                h.lines.push(('-', Some(old), None, body));
                old += 1;
            }
            Some('+') => {
                h.lines.push(('+', None, Some(new), body));
                new += 1;
            }
            // "\ No newline at end of file", or a blank context line some tools trim.
            None => {
                h.lines.push((' ', Some(old), Some(new), String::new()));
                old += 1;
                new += 1;
            }
            _ => {}
        }
    }
    out
}

async fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(dir, args).await?;
    if !out.status.success() {
        return Err(format!(
            "git: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// `git diff --no-index` exits 1 when the files differ, which they always do here.
async fn git_any(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = run(dir, args).await?;
    if out.status.code().is_none_or(|c| c > 1) {
        return Err(format!(
            "git: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn run(dir: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .map_err(|e| format!("git: {e}"))
}

#[cfg(test)]
#[path = "diff_tests.rs"]
mod tests;
