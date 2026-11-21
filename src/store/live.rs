//! Lock-screen card registrations: which devices want the board, and how to reach their card.

use tokio_rusqlite::rusqlite::{OptionalExtension, params};

use super::{PushEnv, PushProvider, Result, Store};

/// What a phone last told the hub about its card.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LiveRegistration {
    pub live_key: Option<String>,
    pub start_token: Option<String>,
    pub activity_token: Option<String>,
    pub activity_started_at: Option<i64>,
    /// The card shows always, not only while something works or waits.
    pub always: bool,
}

/// An active device with a card, and its push registration.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveTarget {
    pub device_id: String,
    pub reg: LiveRegistration,
    /// The app's ordinary push token; Android's card is woken through it.
    pub push_token: Option<String>,
    pub push_env: Option<PushEnv>,
    pub push_provider: PushProvider,
}

impl Store {
    /// Replaces the device's registration; only an active device can register.
    pub async fn set_live(&self, device_id: &str, reg: LiveRegistration, now: i64) -> Result<bool> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            let active: bool = c
                .query_row(
                    "SELECT 1 FROM devices WHERE id = ?1 AND state = 'active'",
                    [&device_id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            if !active {
                return Ok(false);
            }
            c.execute(
                "INSERT INTO live_registrations (device_id, live_key, start_token, activity_token,
                     activity_started_at, updated_at, mode) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (device_id) DO UPDATE SET live_key = ?2, start_token = ?3,
                     activity_token = ?4, activity_started_at = ?5, updated_at = ?6, mode = ?7",
                params![
                    device_id,
                    reg.live_key,
                    reg.start_token,
                    reg.activity_token,
                    reg.activity_started_at,
                    now,
                    if reg.always { "always" } else { "working" }
                ],
            )?;
            Ok(true)
        })
        .await
    }

    /// Stops sending to the device's card.
    pub async fn clear_live(&self, device_id: &str) -> Result<bool> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            Ok(c.execute(
                "DELETE FROM live_registrations WHERE device_id = ?1",
                [device_id],
            )? == 1)
        })
        .await
    }

    /// Forgets a token APNs no longer accepts, if it is still the one on record.
    pub async fn drop_live_token(&self, device_id: &str, token: &str) -> Result<()> {
        let (device_id, token) = (device_id.to_owned(), token.to_owned());
        self.call(move |c| {
            c.execute(
                "UPDATE live_registrations SET activity_token = NULL, activity_started_at = NULL
                 WHERE device_id = ?1 AND activity_token = ?2",
                params![device_id, token],
            )?;
            c.execute(
                "UPDATE live_registrations SET start_token = NULL
                 WHERE device_id = ?1 AND start_token = ?2",
                params![device_id, token],
            )?;
            Ok(())
        })
        .await
    }

    /// Records a card the hub started through the push-to-start token.
    pub async fn live_started(&self, device_id: &str, at: i64) -> Result<()> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            c.execute(
                "UPDATE live_registrations SET activity_started_at = ?2 WHERE device_id = ?1",
                params![device_id, at],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn live_registration(&self, device_id: &str) -> Result<Option<LiveRegistration>> {
        let device_id = device_id.to_owned();
        self.call(move |c| {
            Ok(c.query_row(
                "SELECT live_key, start_token, activity_token, activity_started_at, mode
                 FROM live_registrations WHERE device_id = ?1",
                [device_id],
                |r| {
                    Ok(LiveRegistration {
                        live_key: r.get(0)?,
                        start_token: r.get(1)?,
                        activity_token: r.get(2)?,
                        activity_started_at: r.get(3)?,
                        always: r.get::<_, String>(4)? == "always",
                    })
                },
            )
            .optional()?)
        })
        .await
    }

    /// Every active device with a card.
    pub async fn live_targets(&self) -> Result<Vec<LiveTarget>> {
        self.call(|c| {
            let mut stmt = c.prepare(
                "SELECT l.device_id, l.live_key, l.start_token, l.activity_token,
                        l.activity_started_at, d.push_token, d.push_env, d.push_provider, l.mode
                 FROM live_registrations l JOIN devices d ON d.id = l.device_id
                 WHERE d.state = 'active' ORDER BY l.device_id",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        LiveRegistration {
                            live_key: r.get(1)?,
                            start_token: r.get(2)?,
                            activity_token: r.get(3)?,
                            activity_started_at: r.get(4)?,
                            always: r.get::<_, String>(8)? == "always",
                        },
                        r.get::<_, Option<String>>(5)?,
                        r.get::<_, Option<String>>(6)?,
                        r.get::<_, Option<String>>(7)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            rows.into_iter()
                .map(|(device_id, reg, push_token, env, provider)| {
                    Ok(LiveTarget {
                        device_id,
                        reg,
                        push_token,
                        push_env: env.map(PushEnv::parse).transpose()?,
                        push_provider: provider
                            .map(PushProvider::parse)
                            .transpose()?
                            .unwrap_or(PushProvider::Apns),
                    })
                })
                .collect()
        })
        .await
    }
}
