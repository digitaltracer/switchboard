//! Renaming a space or tab in the app's layout document. Only the one `name` field changes,
//! every other field is kept as read, and the write is compare-and-set on the version.

use serde_json::Value;

use super::types::LAYOUT_DOC;
use super::{Client, HcError};

/// The core caps a document at 1 MiB.
pub const MAX_DOC: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// The document changed since it was read; read it again and retry.
    #[error("the layout changed while it was being written")]
    Conflict,
    #[error("no layout document")]
    Missing,
    #[error("no {0} with that id in the layout")]
    NotFound(&'static str),
    #[error("{0}")]
    Refused(&'static str),
    #[error(transparent)]
    Hc(#[from] HcError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Which<'a> {
    Space(&'a str),
    Tab(&'a str),
}

/// Sets one space's or tab's `name` in `doc`, refusing anything ambiguous.
pub fn set_name(doc: &mut Value, which: Which<'_>, name: &str) -> Result<(), LayoutError> {
    let id = match which {
        Which::Space(id) | Which::Tab(id) => id,
    };
    if id.is_empty() || id.contains('/') {
        return Err(LayoutError::Refused("not an id on this machine"));
    }
    if doc.is_null() {
        return Err(LayoutError::Missing);
    }
    let spaces = doc
        .get_mut("spaces")
        .and_then(Value::as_array_mut)
        .ok_or(LayoutError::Missing)?;
    let mut found: Vec<&mut Value> = match which {
        Which::Space(_) => spaces
            .iter_mut()
            .filter(|s| s.get("id").and_then(Value::as_str) == Some(id))
            .collect(),
        Which::Tab(_) => spaces
            .iter_mut()
            .filter_map(|s| s.get_mut("tabs").and_then(Value::as_array_mut))
            .flatten()
            .filter(|t| t.get("id").and_then(Value::as_str) == Some(id))
            .collect(),
    };
    let what = match which {
        Which::Space(_) => "space",
        Which::Tab(_) => "tab",
    };
    match found.len() {
        0 => return Err(LayoutError::NotFound(what)),
        1 => {}
        _ => return Err(LayoutError::Refused("the id appears more than once")),
    }
    let node = found
        .pop()
        .and_then(Value::as_object_mut)
        .ok_or(LayoutError::Refused("not an object"))?;
    node.insert("name".into(), Value::String(name.to_owned()));
    let size = serde_json::to_vec(doc).map_or(usize::MAX, |b| b.len());
    if size > MAX_DOC {
        return Err(LayoutError::Refused("the layout would exceed 1 MiB"));
    }
    Ok(())
}

/// Reads the layout, renames, and writes it back if no one else wrote in between.
pub async fn rename(client: &Client, which: Which<'_>, name: &str) -> Result<(), LayoutError> {
    let doc = client.doc_get(LAYOUT_DOC).await?;
    if doc.version == 0 {
        return Err(LayoutError::Missing);
    }
    let mut value = doc.value;
    set_name(&mut value, which, name)?;
    match client.doc_set(LAYOUT_DOC, &value, doc.version).await {
        Ok(_) => Ok(()),
        Err(e) if e.kind() == Some("conflict") => Err(LayoutError::Conflict),
        Err(e) => Err(e.into()),
    }
}

pub async fn rename_tab(client: &Client, tab_id: &str, name: &str) -> Result<(), LayoutError> {
    rename(client, Which::Tab(tab_id), name).await
}

pub async fn rename_space(client: &Client, space_id: &str, name: &str) -> Result<(), LayoutError> {
    rename(client, Which::Space(space_id), name).await
}
