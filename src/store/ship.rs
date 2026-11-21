//! Ship buttons, kept on the hub so every paired phone shows the same ones.

use tokio_rusqlite::rusqlite::params;

use super::{Result, Store};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShipButton {
    pub skill: String,
    pub label: String,
    pub after_pr: bool,
}

impl Store {
    /// Every scope's buttons, in order: `*` and each project with its own list.
    pub async fn ship_buttons(&self) -> Result<Vec<(String, Vec<ShipButton>)>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT scope, skill, label, after_pr FROM ship_buttons ORDER BY scope, position",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        ShipButton {
                            skill: r.get(1)?,
                            label: r.get(2)?,
                            after_pr: r.get(3)?,
                        },
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut out: Vec<(String, Vec<ShipButton>)> = Vec::new();
            for (scope, b) in rows {
                match out.last_mut() {
                    Some((s, list)) if *s == scope => list.push(b),
                    _ => out.push((scope, vec![b])),
                }
            }
            Ok(out)
        })
        .await
    }

    /// Replaces one scope's list; an empty list removes it.
    pub async fn set_ship_buttons(&self, scope: &str, buttons: Vec<ShipButton>) -> Result<()> {
        let scope = scope.to_owned();
        self.call(move |c| {
            let tx = c.transaction()?;
            tx.execute("DELETE FROM ship_buttons WHERE scope = ?1", [&scope])?;
            for (i, b) in buttons.iter().enumerate() {
                tx.execute(
                    "INSERT INTO ship_buttons (scope, position, skill, label, after_pr)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![scope, i as i64, b.skill, b.label, b.after_pr],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
        .await
    }
}
