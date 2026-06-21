use crate::{Forwarder, ForwarderError, Result};
use prost::Message;
use public::proto::agent::{
    GenesisSyncRequest, GenesisSyncResponse, GpidSyncRequest, GpidSyncResponse,
    KubernetesApiSyncRequest, KubernetesApiSyncResponse, KubernetesClusterIdRequest,
    KubernetesClusterIdResponse, NtpRequest, NtpResponse, RemoteExecRequest, RemoteExecResponse,
    SyncRequest, SyncResponse,
};

impl Forwarder {
    /// Config sync — POST a `SyncRequest`, receive a `SyncResponse`. Replaces the gRPC
    /// `Synchronizer.Sync`; the server reuses the exact trisolaris processing logic.
    /// Periodic calls also serve as registration + heartbeat (as in the gRPC model).
    pub async fn sync(&self, req: &SyncRequest) -> Result<SyncResponse> {
        self.post_proto("/api/v1/agent/sync", req).await
    }

    /// Clock-offset (NTP) query.
    pub async fn query_time(&self, req: &NtpRequest) -> Result<NtpResponse> {
        self.post_proto("/api/v1/agent/ntp", req).await
    }

    /// Global PID sync.
    pub async fn gpid_sync(&self, req: &GpidSyncRequest) -> Result<GpidSyncResponse> {
        self.post_proto("/api/v1/agent/gpid_sync", req).await
    }

    /// Genesis platform-data sync.
    pub async fn genesis_sync(&self, req: &GenesisSyncRequest) -> Result<GenesisSyncResponse> {
        self.post_proto("/api/v1/agent/genesis_sync", req).await
    }

    /// Kubernetes API resource sync.
    pub async fn kubernetes_api_sync(
        &self,
        req: &KubernetesApiSyncRequest,
    ) -> Result<KubernetesApiSyncResponse> {
        self.post_proto("/api/v1/agent/kubernetes_api_sync", req).await
    }

    /// Resolve / report the Kubernetes cluster id.
    pub async fn kubernetes_cluster_id(
        &self,
        req: &KubernetesClusterIdRequest,
    ) -> Result<KubernetesClusterIdResponse> {
        self.post_proto("/api/v1/agent/kubernetes_cluster_id", req).await
    }

    /// Long-poll for the next remote-exec command (replaces the server->agent half of
    /// the RemoteExecute bidi stream). The agent posts its identity/heartbeat as a
    /// `RemoteExecResponse`; `Ok(None)` means the long-poll expired with no command
    /// queued (the agent should re-poll).
    pub async fn remote_exec_poll(
        &self,
        info: &RemoteExecResponse,
    ) -> Result<Option<RemoteExecRequest>> {
        let resp = self
            .authed(self.client.post(self.url("/api/v1/agent/remote_exec/poll")))
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(info.encode_to_vec())
            .send()
            .await?;
        let status = resp.status();
        if status.as_u16() == 204 {
            return Ok(None); // nothing queued
        }
        if !status.is_success() {
            return Err(ForwarderError::Status(status.as_u16()));
        }
        let bytes = resp.bytes().await?;
        Ok(Some(RemoteExecRequest::decode(bytes)?))
    }

    /// Submit a remote-exec command result (replaces the agent->server half).
    pub async fn remote_exec_result(&self, result: &RemoteExecResponse) -> Result<()> {
        let resp = self
            .authed(self.client.post(self.url("/api/v1/agent/remote_exec/result")))
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(result.encode_to_vec())
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ForwarderError::Status(status.as_u16()));
        }
        Ok(())
    }

    // TODO(M1): upgrade / plugin are server-streaming downloads — the server replies
    // with a sequence of length-delimited protobuf frames. They need a frame-reading
    // helper rather than post_proto; deferred until the agent's upgrade/plugin loaders
    // are ported to the new architecture.

    /// Shared protobuf request/response round-trip used by the unary control endpoints.
    async fn post_proto<Req, Resp>(&self, path: &str, req: &Req) -> Result<Resp>
    where
        Req: Message,
        Resp: Message + Default,
    {
        let resp = self
            .authed(self.client.post(self.url(path)))
            .header(reqwest::header::CONTENT_TYPE, "application/x-protobuf")
            .body(req.encode_to_vec())
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ForwarderError::Status(status.as_u16()));
        }
        let bytes = resp.bytes().await?;
        Ok(Resp::decode(bytes)?)
    }
}
