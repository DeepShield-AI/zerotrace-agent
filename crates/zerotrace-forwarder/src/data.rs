use crate::{Forwarder, ForwarderError, Result};

impl Forwarder {
    /// Upload one batch of agent wire-format frames (BaseHeader[+FlowHeader]+protobuf)
    /// to `/api/v1/data/ingest`. The server feeds them into the ingester pipeline, so
    /// rows land in `flow_log.*` exactly as the legacy TCP path produced them.
    ///
    /// `frames` is the same byte stream the TCP `uniform_sender` would have written
    /// (already compressed per the per-frame `FlowHeader.Encoder`).
    pub async fn upload_frames(&self, frames: impl Into<Vec<u8>>) -> Result<()> {
        let resp = self
            .authed(self.client.post(self.url("/api/v1/data/ingest")))
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(frames.into())
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ForwarderError::Status(status.as_u16()));
        }
        Ok(())
    }
}
