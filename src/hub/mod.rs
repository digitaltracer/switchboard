//! Hub: the runner WebSocket endpoint, the session service and the audit log. MCP and OAuth
//! (step 7) mount on the same router and call the session service.

pub mod runners;
pub mod service;
pub mod ws;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::config::HubConfig;
use crate::store::Store;
use service::{Clock, SessionService};

pub async fn run(config: PathBuf) -> anyhow::Result<()> {
    let cfg = HubConfig::load(&config)?;
    let store = Store::open(&cfg.database)
        .await
        .with_context(|| format!("opening {}", cfg.database.display()))?;
    let listen = cfg.listen;
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
    let hub = serve(svc, listener)?;
    tracing::info!(addr = %hub.addr, "hub listening");
    tokio::signal::ctrl_c().await?;
    hub.shutdown().await;
    Ok(())
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
    let app = ws::routes(svc.clone());
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
