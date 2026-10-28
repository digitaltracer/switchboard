use super::*;

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

#[test]
fn claude_lists_the_repos_yours_plugins_and_built_ins_once_each() {
    let home = tempfile::tempdir().unwrap();
    let dir = home.path().join(".claude");
    let repo = home.path().join("repo");
    write(
        &dir.join("skills/council/SKILL.md"),
        "---\nname: council\ndescription: >\n  Several agents review a plan\n  and merge findings.\n---\n# Council\n",
    );
    write(
        &dir.join("skills/.trash/old/SKILL.md"),
        "---\nname: old\n---\n",
    );
    write(
        &dir.join("skills/hidden/SKILL.md"),
        "---\nname: hidden\nuser-invocable: false\ndescription: no\n---\n",
    );
    write(
        &dir.join("commands/commit.md"),
        "---\neffort: high\n---\n# Claude Command: Commit\n\nCreate well-formatted commits.\n",
    );
    write(&dir.join("commands/git/push.md"), "Push the branch.\n");
    write(
        &repo.join(".claude/skills/council/SKILL.md"),
        "---\nname: council\ndescription: The repo's own council\n---\n",
    );
    let plugin = home.path().join("plugins/cache/swift-lsp");
    write(
        &plugin.join("skills/format/SKILL.md"),
        "---\nname: format\ndescription: Format Swift\n---\n",
    );
    write(
        &dir.join("plugins/installed_plugins.json"),
        &serde_json::json!({"version": 2, "plugins": {"swift-lsp@official": [{"installPath": plugin}]}})
            .to_string(),
    );

    let list = claude(&dir, Some(&repo));
    let shown: Vec<_> = list
        .iter()
        .map(|s| (s.name.as_str(), s.description.as_str(), s.source.as_str()))
        .collect();
    assert_eq!(
        shown,
        vec![
            ("/council", "The repo's own council", "repo"),
            ("/commit", "Create well-formatted commits.", "user"),
            ("/git:push", "Push the branch.", "user"),
            ("/swift-lsp:format", "Format Swift", "plugin"),
            ("/clear", "Start a new conversation", "builtin"),
            (
                "/compact",
                "Summarise the conversation to free up context",
                "builtin"
            ),
        ]
    );
}

#[test]
fn codex_skills_go_by_dollar_and_prompts_by_their_namespace() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join("skills/linear/SKILL.md"),
        "---\nname: linear\ndescription: Manage Linear issues\nmetadata:\n  short-description: x\n---\n",
    );
    write(
        &home.path().join("prompts/review.md"),
        "---\ndescription: Review the diff\n---\n",
    );
    let names: Vec<_> = codex(home.path()).into_iter().map(|s| s.name).collect();
    assert_eq!(
        names,
        vec!["$linear", "/prompts:review", "/new", "/compact"]
    );
}

#[test]
fn long_descriptions_are_cut() {
    let home = tempfile::tempdir().unwrap();
    write(
        &home.path().join("skills/long/SKILL.md"),
        &format!(
            "---\nname: long\ndescription: {}\n---\n",
            "word ".repeat(80)
        ),
    );
    let list = claude(home.path(), None);
    assert!(list[0].description.ends_with('…') && list[0].description.chars().count() <= 161);
}
