//! # zerotrace-forwarder
//!
//! The agent's single HTTP short-connection outlet to the ZeroTrace server. It
//! replaces the legacy gRPC Synchronizer stream (control plane) and the raw TCP
//! uniform_sender stream (data plane) with plain agent-initiated HTTP:
//!
//! * **control plane** — protobuf request/response against `/api/v1/agent/*`
//!   (`sync`, `query_time`, …); the server reuses the exact trisolaris logic.
//! * **data plane** — a wire-frame firehose to `/api/v1/data/ingest`; the bytes are
//!   identical to what the TCP path sent, so the server feeds them through the same
//!   ingester pipeline and rows land in `flow_log.*` byte-for-byte.
//!
//! Naming: this is a *forwarder* (à la Datadog), not an RPC client — HTTP short
//! connections are not procedure calls and the crate also carries the data firehose.

mod auth;
mod config;
mod control;
mod data;
mod error;

pub use config::ForwarderConfig;
pub use error::{ForwarderError, Result};
use std::time::Duration;

/// HTTP forwarder to the ZeroTrace server. Cheap to clone (wraps a `reqwest::Client`
/// which is itself an `Arc` internally); construct once and share.
#[derive(Clone)]
pub struct Forwarder {
    client: reqwest::Client,
    cfg: ForwarderConfig,
}

impl Forwarder {
    pub fn new(cfg: ForwarderConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(cfg.timeout)
            .http2_prior_knowledge()
            .build()?;
        Ok(Self { client, cfg })
    }

    pub fn builder() -> ForwarderBuilder {
        ForwarderBuilder::default()
    }

    /// Join the configured base URL with an endpoint path.
    pub(crate) fn url(&self, path: &str) -> String {
        format!("{}{}", self.cfg.base_url.trim_end_matches('/'), path)
    }
}

/// Ergonomic builder; `api_key_from_env_or` mirrors Datadog's `DD_API_KEY` override.
#[derive(Default)]
pub struct ForwarderBuilder {
    cfg: ForwarderConfig,
}

impl ForwarderBuilder {
    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.cfg.base_url = url.into();
        self
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.cfg.api_key = key.into();
        self
    }

    /// Take the API key from the `ZT_API_KEY` env var, falling back to `fallback`.
    pub fn api_key_from_env_or(mut self, fallback: impl Into<String>) -> Self {
        self.cfg.api_key = std::env::var("ZT_API_KEY").unwrap_or_else(|_| fallback.into());
        self
    }

    pub fn agent_id(mut self, id: impl Into<String>) -> Self {
        self.cfg.agent_id = Some(id.into());
        self
    }

    pub fn timeout(mut self, d: Duration) -> Self {
        self.cfg.timeout = d;
        self
    }

    pub fn build(self) -> Result<Forwarder> {
        Forwarder::new(self.cfg)
    }
}
