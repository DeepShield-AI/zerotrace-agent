use std::time::Duration;

use zerotrace_forwarder::{Forwarder, ForwarderError};

/// HTTP reporter: ships agent wire-format frames to the server over HTTP short
/// connections via `zerotrace-forwarder`.
///
/// The forwarder is async (reqwest), but the current data path (`uniform_sender`)
/// is a synchronous thread, so this holds a dedicated current-thread Tokio runtime
/// and `block_on`s each upload. When the pipeline becomes async (M0/M1) the reporter
/// will simply `.await` the forwarder and this runtime goes away.
pub struct HttpForwarder {
    forwarder: Forwarder,
    rt: tokio::runtime::Runtime,
}

impl HttpForwarder {
    pub fn new(
        base_url: String,
        api_key: String,
        agent_id: Option<String>,
    ) -> Result<Self, String> {
        let mut builder = Forwarder::builder()
            .base_url(base_url)
            .api_key(api_key)
            .timeout(Duration::from_secs(10));
        if let Some(id) = agent_id {
            builder = builder.agent_id(id);
        }
        let forwarder = builder.build().map_err(|e| e.to_string())?;
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { forwarder, rt })
    }

    /// Synchronously upload one batch of wire frames (blocks the calling thread until
    /// the HTTP request completes).
    pub fn upload(&self, frames: &[u8]) -> Result<(), ForwarderError> {
        self.rt.block_on(self.forwarder.upload_frames(frames.to_vec()))
    }
}
