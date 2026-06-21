use crate::Forwarder;
use reqwest::RequestBuilder;

impl Forwarder {
    /// Attach the `X-Api-Key` (and optional `X-Agent-Id`) authentication headers,
    /// matching the server's `APIKeyAuth` middleware on `/api/v1`.
    pub(crate) fn authed(&self, rb: RequestBuilder) -> RequestBuilder {
        let mut rb = rb.header("X-Api-Key", self.cfg.api_key.as_str());
        if let Some(id) = &self.cfg.agent_id {
            rb = rb.header("X-Agent-Id", id.as_str());
        }
        rb
    }
}
