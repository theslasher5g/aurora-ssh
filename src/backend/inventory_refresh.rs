//! Server inventory: collects the host facts in the background over a separate SSH
//! connection (stored credentials only – nothing here ever asks for a password).

use std::time::Duration;

use uuid::Uuid;

use super::inventory_collect::{Collected, collect};
use super::inventory_info::InfoSnippet;
use super::ssh::{SshParams, connect};
use super::{HostKeyPrompt, Sink};

/// How many hosts are refreshed at the same time.
pub const PARALLEL: usize = 4;

pub struct Done {
    pub host: Uuid,
    pub result: Result<Collected, String>,
}

pub fn spawn(
    rt: &tokio::runtime::Runtime,
    host: Uuid,
    params: SshParams,
    snippets: Vec<InfoSnippet>,
    sink: Sink<Done>,
    prompts: Sink<HostKeyPrompt>,
) {
    rt.spawn(async move {
        let run = async {
            let handle = connect(&params, prompts)
                .await
                .map_err(|e| format!("{e:#}"))?;
            let facts = collect(&handle, &snippets).await;
            let _ = handle
                .disconnect(russh::Disconnect::ByApplication, "bye", "en")
                .await;
            facts.ok_or_else(|| t!("Der Server hat keine Fakten geliefert").to_string())
        };
        let result = match tokio::time::timeout(Duration::from_secs(110), run).await {
            Ok(r) => r,
            Err(_) => Err(t!("Zeitüberschreitung").to_string()),
        };
        sink.send(Done { host, result });
    });
}
