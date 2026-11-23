//! herdr-core's records, as far as Switchboard reads them. Unknown fields are ignored, and
//! every field the core may leave out has a default.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CoreInfo {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The reply to `hello`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Hello {
    pub generation: u32,
    pub version: String,
    pub core: CoreInfo,
    #[serde(default)]
    pub methods: Vec<String>,
    #[serde(default)]
    pub features: Vec<String>,
}

/// One OSC 7501 record: the program's own words.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Status {
    /// `idle`, `working`, `done`, `blocked` or `error`.
    pub state: String,
    #[serde(default)]
    pub app: Option<String>,
    /// `permission`, `question` or `auth`, when blocked and the program said.
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub msg: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Foreground {
    pub pid: i64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
pub struct Terminal {
    /// `t7`; one containing `/` is another machine's.
    pub address: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub created_ms: u64,
    /// `live` or `finished`.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub reported: bool,
    #[serde(default)]
    pub status: Option<Status>,
    #[serde(default)]
    pub status_ms: Option<u64>,
    #[serde(default)]
    pub turn: u64,
    #[serde(default)]
    pub foreground: Option<Foreground>,
    #[serde(default)]
    pub parent: Option<String>,
}

impl Terminal {
    pub fn is_live(&self) -> bool {
        self.state == "live"
    }

    /// This machine's terminal, not one reached through `hc machine`.
    pub fn is_local(&self) -> bool {
        !self.address.contains('/')
    }

    /// Who the agent is: the same address can later hold a shell or another program.
    pub fn identity(&self, core_id: &str) -> String {
        format!(
            "{core_id}:{}:{}:{}",
            self.address,
            self.pid.unwrap_or_default(),
            self.created_ms
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ReadResult {
    pub text: String,
    #[serde(default = "yes")]
    pub complete: bool,
    #[serde(default)]
    pub how: String,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SendResult {
    pub turn: u64,
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct KeysResult {
    pub turn: u64,
    #[serde(default)]
    pub status: Option<String>,
}

/// An opaque document and its compare-and-set version; zero when never written.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Doc {
    pub value: Value,
    pub version: u64,
}

/// What `terminal.run` is asked for. `force`-style options don't exist here on purpose.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct RunParams {
    pub argv: Vec<String>,
    pub cwd: String,
    pub name: String,
    pub labels: BTreeMap<String, String>,
    pub keep: bool,
    pub no_parent: bool,
}

/// The app's layout document's name on the core.
pub const LAYOUT_DOC: &str = "herdr-app";

/// A space in the layout document, as Switchboard reads it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SpaceView {
    pub id: String,
    pub name: Option<String>,
    pub folder: Option<String>,
    /// The machine it lives on, when that isn't this one.
    pub machine: Option<String>,
    pub tabs: Vec<TabView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TabView {
    pub id: String,
    pub name: Option<String>,
    /// Addresses of the terminals placed in the tab's panes.
    pub terminals: Vec<String>,
}

/// The spaces and tabs of a layout document; anything unexpected is skipped, not an error.
pub fn spaces(doc: &Value) -> Vec<SpaceView> {
    let text = |v: &Value, k: &str| {
        v.get(k)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let Some(list) = doc.get("spaces").and_then(Value::as_array) else {
        return Vec::new();
    };
    list.iter()
        .filter_map(|s| {
            Some(SpaceView {
                id: text(s, "id")?,
                name: text(s, "name"),
                folder: text(s, "folder"),
                machine: text(s, "machine"),
                tabs: s
                    .get("tabs")
                    .and_then(Value::as_array)
                    .map(|tabs| {
                        tabs.iter()
                            .filter_map(|t| {
                                let mut terminals = Vec::new();
                                if let Some(root) = t.get("root") {
                                    collect_terminals(root, &mut terminals);
                                }
                                Some(TabView {
                                    id: text(t, "id")?,
                                    name: text(t, "name"),
                                    terminals,
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
            })
        })
        .collect()
}

fn collect_terminals(node: &Value, out: &mut Vec<String>) {
    if let Some(t) = node.pointer("/content/terminal").and_then(Value::as_str) {
        out.push(t.to_owned());
    }
    for child in node
        .get("children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        collect_terminals(child, out);
    }
}
