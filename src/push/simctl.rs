//! Delivers to a simulator on this Mac with `xcrun simctl push`, for testing without Apple's
//! servers or a developer account. Every paired device's notifications go to that simulator.

use std::process::Stdio;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use super::{Push, SendOutcome, Sender};

const TIMEOUT: Duration = Duration::from_secs(20);

pub struct Simctl {
    simulator: String,
    bundle_id: String,
}

impl Simctl {
    pub fn new(simulator: &str, bundle_id: &str) -> Self {
        Self {
            simulator: simulator.to_owned(),
            bundle_id: bundle_id.to_owned(),
        }
    }

    /// The command line, without the payload, which goes on stdin.
    pub fn args(&self) -> Vec<String> {
        ["simctl", "push", &self.simulator, &self.bundle_id, "-"]
            .map(str::to_owned)
            .to_vec()
    }
}

impl Sender for Simctl {
    fn needs_token(&self) -> bool {
        false
    }

    async fn send(&self, push: &Push) -> SendOutcome {
        let run = async {
            let mut child = tokio::process::Command::new("xcrun")
                .args(self.args())
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()?;
            let mut stdin = child.stdin.take().expect("piped");
            stdin.write_all(push.payload.to_string().as_bytes()).await?;
            drop(stdin);
            child.wait_with_output().await
        };
        match tokio::time::timeout(TIMEOUT, run).await {
            Ok(Ok(out)) if out.status.success() => SendOutcome::Sent,
            Ok(Ok(out)) => SendOutcome::Retry(format!(
                "simctl push: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            )),
            Ok(Err(e)) => SendOutcome::Retry(format!("running simctl: {e}")),
            Err(_) => SendOutcome::Retry("simctl push timed out".into()),
        }
    }
}
