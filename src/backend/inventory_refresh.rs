//! Server inventory: collects the host facts in the background over a separate SSH
//! connection (stored credentials only – nothing here ever asks for a password).

use std::time::Duration;

use uuid::Uuid;

use super::ssh::{SshParams, collect_facts, connect};
use super::{HostKeyPrompt, Sink};
use crate::model::HostFacts;

/// How many hosts are refreshed at the same time.
pub const PARALLEL: usize = 4;

pub struct Done {
    pub host: Uuid,
    pub result: Result<HostFacts, String>,
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    host: Uuid,
    params: SshParams,
    sink: Sink<Done>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let run = async {
            let handle = connect(&params, prompts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let facts = collect_facts(&handle).await;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            facts.ok_or_else(|| t!("Der Server hat keine Fakten geliefert").to_string())
        };
        let result = match tokio::time::timeout(Duration::from_secs(75), run).await {
            Ok(r) => r,
            Err(_) => Err(t!("Zeitüberschreitung").to_string()),
        };
        sink.send(Done { host, result });
    });
}
