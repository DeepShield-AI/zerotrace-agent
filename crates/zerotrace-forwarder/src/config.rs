use std::time::Duration;

/// Forwarder connection settings. Defaults target a local controller over plain HTTP.
#[derive(Clone, Debug)]
pub struct ForwarderConfig {
    /// Controller HTTP base URL, e.g. `http://127.0.0.1:30417`.
    pub base_url: String,
    /// API key presented in the `X-Api-Key` header.
    pub api_key: String,
    /// Optional agent id sent in `X-Agent-Id`.
    pub agent_id: Option<String>,
    /// Per-request timeout.
    pub timeout: Duration,
    /// Max retries on transient failure (used by the retry helper; see TODO in data.rs).
    pub retries: u32,
}

impl Default for ForwarderConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:30417".to_string(),
            api_key: String::new(),
            agent_id: None,
            timeout: Duration::from_secs(10),
            retries: 3,
        }
    }
}
