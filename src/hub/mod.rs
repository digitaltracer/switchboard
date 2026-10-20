//! Hub: the runner WebSocket endpoint, the session service and the audit log. MCP and OAuth
//! (step 7) mount on the same router and call the session service.

pub mod app;
pub mod runners;
pub mod service;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::auth::device::Devices;
use crate::config::HubConfig;
use crate::store::Store;
use service::{Clock, SessionService};

pub async fn run(config: PathBuf, whatsapp_pair_phone: Option<String>) -> anyhow::Result<()> {
    let cfg = HubConfig::load(&config)?;
    let whatsapp = cfg.whatsapp.clone();
    if whatsapp.is_none() && whatsapp_pair_phone.is_some() {
        anyhow::bail!(
            "--whatsapp-pair-phone needs a [whatsapp] section in {}",
            config.display()
        );
    }
    let store = Store::open(&cfg.database)
        .await
        .with_context(|| format!("opening {}", cfg.database.display()))?;
    let listen = cfg.listen;
    let push = cfg.push.clone();
    tracing::info!(
        projects = cfg.projects.len(),
        machines = cfg.machines.len(),
        "hub config loaded"
    );
    let svc = SessionService::new(cfg, store, Clock::System);
    svc.recover().await?;
    let listener = TcpListener::bind(listen)
        .await
        .with_context(|| format!("binding {listen}"))?;
    let hub = serve(svc.clone(), listener)?;
    tracing::info!(addr = %hub.addr, "hub listening");
    let _push = match &push {
        Some(p) => {
            tracing::info!(sender = ?p.sender, "push notifications on");
            Some(AbortOnDrop(crate::push::start(
                p,
                svc.store().clone(),
                Clock::System,
            )?))
        }
        None => None,
    };
    let Some(wa) = whatsapp else {
        tokio::signal::ctrl_c().await?;
        hub.shutdown().await;
        return Ok(());
    };
    let mut bridge = tokio::spawn(crate::whatsapp::run(svc, wa, whatsapp_pair_phone));
    tokio::select! {
        stop = tokio::signal::ctrl_c() => {
            hub.shutdown().await;
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), bridge).await;
            Ok(stop?)
        }
        // A bridge that can't go on (unlinked, banned, wrong account) stops the hub with its reason.
        end = &mut bridge => {
            hub.shutdown().await;
            end?
        }
    }
}

/// Stops a background task when the hub stops.
struct AbortOnDrop(JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

pub struct Hub {
    pub addr: SocketAddr,
    pub service: Arc<SessionService>,
    server: JoinHandle<std::io::Result<()>>,
    sweeper: JoinHandle<()>,
}

/// Serves the runner endpoint on `listener` and starts the approval sweeper.
pub fn serve(svc: Arc<SessionService>, listener: TcpListener) -> anyhow::Result<Hub> {
    let addr = listener.local_addr()?;
    let devices = Arc::new(Devices::new(svc.store().clone(), svc.clock().clone()));
    let app = ws::routes(svc.clone()).merge(app::routes(app::AppState {
        svc: svc.clone(),
        devices,
    }));
    let mut stop = svc.shutdown_signal();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(async move {
                let _ = stop.wait_for(|s| *s).await;
            })
            .await
    });
    Ok(Hub {
        addr,
        sweeper: svc.spawn_sweeper(),
        service: svc,
        server,
    })
}

impl Hub {
    /// Closes runner connections and stops serving; records stay in the database.
    pub async fn shutdown(self) {
        self.service.begin_shutdown();
        self.sweeper.abort();
        if let Err(e) = self.server.await {
            tracing::warn!(error = %e, "hub server task");
        }
    }
}
