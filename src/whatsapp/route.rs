//! Where a message goes: projects named in free text, machines, and sessions or agents named
//! by `use`. Plain word matching against the hub's allowlist; no language model.

use crate::hub::service::ProjectView;

/// A project name as the bridge compares it: lowercase, with `_`, `/` and `.` read as `-`,
/// so `personal/foo` matches the id `personal-foo`.
pub fn norm(name: &str) -> String {
    name.trim_matches(|c: char| matches!(c, '-' | '_' | '/' | '.'))
        .chars()
        .map(|c| match c {
            '_' | '/' | '.' => '-',
            c => c.to_ascii_lowercase(),
        })
        .collect()
}

/// The message's words, normalised like project names.
pub fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '/' | '.')))
        .map(norm)
        .filter(|w| !w.is_empty())
        .collect()
}

fn names(p: &ProjectView) -> impl Iterator<Item = String> + '_ {
    std::iter::once(&p.id).chain(&p.aliases).map(|n| norm(n))
}

/// The project whose id or alias is exactly `name`.
pub fn project_by_name<'a>(name: &str, projects: &'a [ProjectView]) -> Option<&'a ProjectView> {
    let name = norm(name);
    projects.iter().find(|p| names(p).any(|n| n == name))
}

/// Project ids named in `text`, in order of appearance. A leading "in <project>" (or "on",
/// "for") settles it on its own, so "in switchboard, fix the hub test" isn't also `hub`.
pub fn projects_named(text: &str, projects: &[ProjectView]) -> Vec<String> {
    let words = words(text);
    if let [lead, first, ..] = words.as_slice()
        && matches!(lead.as_str(), "in" | "on" | "for")
        && let Some(p) = project_by_name(first, projects)
    {
        return vec![p.id.clone()];
    }
    let mut found: Vec<String> = Vec::new();
    for w in &words {
        if let Some(p) = projects.iter().find(|p| names(p).any(|n| n == *w))
            && !found.contains(&p.id)
        {
            found.push(p.id.clone());
        }
    }
    found
}

/// Configured machine ids that appear as words in `text`.
pub fn machines_named(text: &str, machines: &[String]) -> Vec<String> {
    let words = words(text);
    machines
        .iter()
        .filter(|m| words.contains(&norm(m)))
        .cloned()
        .collect()
}

/// A session's short label: its project and the first four characters of its id.
pub fn label(project_id: &str, session_id: &str) -> String {
    format!("{project_id}-{}", &session_id[..session_id.len().min(4)])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Session {
        id: String,
        label: String,
        machine: String,
        agent: Option<String>,
    },
    /// A named Herdr agent Switchboard didn't create.
    Agent { machine: String, name: String },
}

impl Target {
    pub fn label(&self) -> &str {
        match self {
            Target::Session { label, .. } => label,
            Target::Agent { name, .. } => name,
        }
    }

    pub fn agent_name(&self) -> Option<&str> {
        match self {
            Target::Session { agent, .. } => agent.as_deref(),
            Target::Agent { name, .. } => Some(name),
        }
    }
}

/// Something running that `use <x>` can pick, with every name it answers to.
#[derive(Debug, Clone)]
pub struct Live {
    pub target: Target,
    /// Normalised: for a session its id, label, branch, agent name, project id and aliases.
    pub names: Vec<String>,
    pub status: String,
    /// What a listing shows after the label.
    pub detail: String,
    /// Extra lines for `status`, such as pending approvals.
    pub notes: Vec<String>,
}

/// Live entries `x` names: exact names first; failing those, names that start with `x`.
pub fn match_target(x: &str, live: &[Live]) -> Vec<Target> {
    let x = norm(x.trim_start_matches('#'));
    if x.is_empty() {
        return vec![];
    }
    let pick = |hit: &dyn Fn(&str) -> bool| -> Vec<Target> {
        live.iter()
            .filter(|l| l.names.iter().any(|n| hit(n)))
            .map(|l| l.target.clone())
            .collect()
    };
    let exact = pick(&|n| n == x);
    if !exact.is_empty() || x.len() < 3 {
        return exact;
    }
    pick(&|n| n.starts_with(&x))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Agent;

    fn project(id: &str, aliases: &[&str]) -> ProjectView {
        ProjectView {
            id: id.into(),
            aliases: aliases.iter().map(|a| (*a).into()).collect(),
            description: "d".into(),
            default_agent: Agent::Claude,
            machines: vec!["mac".into()],
        }
    }

    fn projects() -> Vec<ProjectView> {
        vec![
            project("switchboard", &["sb"]),
            project("hub", &[]),
            project("dpv2-apps-helm", &[]),
            project("personal-foo", &[]),
            project("work-foo", &[]),
            project("lib-utils", &[]),
        ]
    }

    #[test]
    fn projects_by_id_alias_and_case() {
        let p = projects();
        assert_eq!(
            projects_named("fix the build in Switchboard", &p),
            ["switchboard"]
        );
        assert_eq!(projects_named("sb: tidy the README", &p), ["switchboard"]);
        assert_eq!(
            projects_named("bump DPv2-apps-helm chart", &p),
            ["dpv2-apps-helm"]
        );
        assert_eq!(
            projects_named("lib_utils needs a release.", &p),
            ["lib-utils"]
        );
        assert!(projects_named("also add a test", &p).is_empty());
        assert!(
            projects_named("switchboards are old", &p).is_empty(),
            "whole words only"
        );
        assert!(
            projects_named("the hub-runner test", &p).is_empty(),
            "hub-runner is one word"
        );
    }

    #[test]
    fn several_projects_are_all_returned_unless_one_leads() {
        let p = projects();
        assert_eq!(
            projects_named("port the hub fix to switchboard and sb", &p),
            ["hub", "switchboard"]
        );
        assert_eq!(
            projects_named("In switchboard, fix the flaky hub test", &p),
            ["switchboard"]
        );
        assert_eq!(projects_named("on hub: sync with switchboard", &p), ["hub"]);
    }

    #[test]
    fn colliding_folder_names_are_told_apart_by_tree() {
        let p = projects();
        assert_eq!(projects_named("in work/foo, bump deps", &p), ["work-foo"]);
        assert_eq!(
            projects_named("personal-foo and work_foo", &p),
            ["personal-foo", "work-foo"]
        );
        assert!(
            projects_named("fix foo", &p).is_empty(),
            "the bare name is ambiguous, so it isn't an alias"
        );
        assert_eq!(project_by_name("Work/Foo", &p).unwrap().id, "work-foo");
        assert!(project_by_name("foo", &p).is_none());
    }

    #[test]
    fn machines_by_word() {
        let m = vec!["mac".to_string(), "vps".to_string()];
        assert_eq!(machines_named("on mac, run the tests", &m), ["mac"]);
        assert!(machines_named("machine learning", &m).is_empty());
    }

    fn session(id: &str, project: &str, aliases: &[&str]) -> Live {
        let lbl = label(project, id);
        let mut names = vec![
            norm(id),
            norm(&lbl),
            norm(&format!("sb/{id}")),
            format!("sb-{id}"),
        ];
        names.push(project.into());
        names.extend(aliases.iter().map(|a| norm(a)));
        Live {
            target: Target::Session {
                id: id.into(),
                label: lbl,
                machine: "mac".into(),
                agent: Some(format!("sb-{id}")),
            },
            names,
            status: "idle".into(),
            detail: String::new(),
            notes: vec![],
        }
    }

    fn agent(name: &str) -> Live {
        Live {
            target: Target::Agent {
                machine: "mac".into(),
                name: name.into(),
            },
            names: vec![name.into()],
            status: "idle".into(),
            detail: String::new(),
            notes: vec![],
        }
    }

    #[test]
    fn targets_by_label_id_branch_project_and_agent_name() {
        let live = vec![
            session("3f9a12345678", "switchboard", &["sb"]),
            session("77aa00000000", "auth", &[]),
            agent("switchboard-impl"),
        ];
        let labels = |x: &str| -> Vec<String> {
            match_target(x, &live)
                .iter()
                .map(|t| t.label().to_owned())
                .collect()
        };
        assert_eq!(labels("auth"), ["auth-77aa"], "by project");
        assert_eq!(labels("switchboard-3f9a"), ["switchboard-3f9a"], "by label");
        assert_eq!(labels("3f9a"), ["switchboard-3f9a"], "by id prefix");
        assert_eq!(labels("sb/3f9a12345678"), ["switchboard-3f9a"], "by branch");
        assert_eq!(labels("sb"), ["switchboard-3f9a"], "by project alias");
        assert_eq!(
            labels("switchboard-impl"),
            ["switchboard-impl"],
            "an agent Switchboard didn't start"
        );
        assert_eq!(labels("Switchboard-Impl"), ["switchboard-impl"]);
        assert!(labels("nothing").is_empty());
        assert!(labels("sw").is_empty(), "too short to be a prefix");
    }

    #[test]
    fn an_ambiguous_name_returns_every_candidate() {
        let live = vec![
            session("3f9a12345678", "switchboard", &[]),
            session("0b1c12345678", "switchboard", &[]),
            agent("switchboard-impl"),
        ];
        let hits = match_target("switchboard", &live);
        assert_eq!(
            hits.len(),
            2,
            "exact project matches win over the agent's prefix"
        );
        let hits = match_target("switch", &live);
        assert_eq!(hits.len(), 3, "a prefix matches all three");
    }
}
