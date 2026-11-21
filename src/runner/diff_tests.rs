use std::path::Path;
use std::process::Command;

use super::*;

fn git_in(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn head(dir: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

/// A repo with one commit, then a change to it: a modified file, a new one and a binary.
fn repo() -> (tempfile::TempDir, String) {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git_in(p, &["init", "-q"]);
    let old: String = (1..=80).map(|i| format!("line {i}\n")).collect();
    std::fs::write(p.join("a.txt"), &old).unwrap();
    std::fs::write(p.join("keep.txt"), "same\n").unwrap();
    git_in(p, &["add", "."]);
    git_in(p, &["commit", "-qm", "base"]);
    let base = head(p);
    let new = old
        .replace("line 2\n", "line two\n")
        .replace("line 70\n", "line seventy\nextra\n");
    std::fs::write(p.join("a.txt"), new).unwrap();
    std::fs::write(p.join("new.txt"), "hello\nworld\n").unwrap();
    std::fs::write(p.join("img.bin"), [0u8, 1, 2, 3, 0, 9]).unwrap();
    (dir, base)
}

#[tokio::test]
async fn a_changed_file_gives_its_hunks_numbered_on_each_side() {
    let (dir, base) = repo();
    let d = diff(dir.path(), &base, "a.txt", 0).await.unwrap();
    assert_eq!(d["binary"], false);
    assert_eq!(
        d["hunk_count"], 2,
        "two changes 68 lines apart are two hunks"
    );
    let first = &d["hunks"][0];
    assert_eq!(first["old_start"], 1);
    let lines = first["lines"].as_array().unwrap();
    let del = lines.iter().find(|l| l["kind"] == "-").unwrap();
    assert_eq!(
        (
            del["old"].as_u64(),
            del["new"].as_u64(),
            del["text"].as_str()
        ),
        (Some(2), None, Some("line 2"))
    );
    let add = lines.iter().find(|l| l["kind"] == "+").unwrap();
    assert_eq!(
        (
            add["old"].as_u64(),
            add["new"].as_u64(),
            add["text"].as_str()
        ),
        (None, Some(2), Some("line two"))
    );
    assert!(
        lines.len() >= 25,
        "unchanged lines come with each change for the app to fold"
    );
    assert_eq!(d["added"], 3);
    assert_eq!(d["deleted"], 2);
    assert!(d["next"].is_null());
}

#[tokio::test]
async fn new_and_binary_files_are_shown_as_such() {
    let (dir, base) = repo();
    let n = diff(dir.path(), &base, "new.txt", 0).await.unwrap();
    let lines = n["hunks"][0]["lines"].as_array().unwrap();
    assert_eq!(
        lines
            .iter()
            .map(|l| l["text"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["hello", "world"]
    );
    assert!(lines.iter().all(|l| l["kind"] == "+"));
    let b = diff(dir.path(), &base, "img.bin", 0).await.unwrap();
    assert_eq!(
        (b["binary"].as_bool(), b["size"].as_u64()),
        (Some(true), Some(6))
    );
    assert!(b["hunks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn only_a_changed_file_inside_the_worktree_can_be_read() {
    let (dir, base) = repo();
    for bad in [
        "keep.txt",
        "../a.txt",
        "/etc/passwd",
        "nope.txt",
        "",
        "a.txt/..",
        "./a.txt",
    ] {
        assert!(diff(dir.path(), &base, bad, 0).await.is_err(), "{bad:?}");
    }
}

#[tokio::test]
async fn a_long_diff_comes_a_page_of_hunks_at_a_time() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    git_in(p, &["init", "-q"]);
    let old: String = (1..=4000).map(|i| format!("{i}\n")).collect();
    std::fs::write(p.join("big.txt"), &old).unwrap();
    git_in(p, &["add", "."]);
    git_in(p, &["commit", "-qm", "base"]);
    let base = head(p);
    // A change every 100 lines: 40 hunks of about 51 lines each.
    let new: String = (1..=4000)
        .map(|i| {
            if i % 100 == 0 {
                format!("{i}!\n")
            } else {
                format!("{i}\n")
            }
        })
        .collect();
    std::fs::write(p.join("big.txt"), new).unwrap();
    let first = diff(p, &base, "big.txt", 0).await.unwrap();
    let count = first["hunk_count"].as_u64().unwrap();
    let got = first["hunks"].as_array().unwrap().len() as u64;
    assert!(count > got && got > 0, "{got} of {count}");
    let total: usize = first["hunks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["lines"].as_array().unwrap().len())
        .sum();
    assert!(total <= PAGE_LINES);
    let next = first["next"].as_u64().unwrap();
    assert_eq!(next, got);
    let second = diff(p, &base, "big.txt", next as u32).await.unwrap();
    assert!(
        second["hunks"][0]["old_start"].as_u64()
            > first["hunks"][got as usize - 1]["old_start"].as_u64()
    );
}

#[test]
fn hunk_headers_and_no_newline_markers_parse() {
    let text = "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -3,2 +3,3 @@ fn main\n a\n-b\n+c\n+d\n\\ No newline at end of file\n";
    let h = parse(text);
    assert_eq!(h.len(), 1);
    assert_eq!((h[0].old_start, h[0].new_start), (3, 3));
    assert_eq!(h[0].header, "@@ -3,2 +3,3 @@ fn main");
    assert_eq!(
        h[0].lines
            .iter()
            .map(|l| (l.0, l.1, l.2))
            .collect::<Vec<_>>(),
        [
            (' ', Some(3), Some(3)),
            ('-', Some(4), None),
            ('+', None, Some(4)),
            ('+', None, Some(5))
        ]
    );
}
