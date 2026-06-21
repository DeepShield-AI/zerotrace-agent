//! End-to-end check for the data path: construct a real l7 (`AppProtoLogsData`) wire
//! frame — exactly the bytes the agent's `uniform_sender` would emit for an l7 flow
//! log — and POST it through `zerotrace-forwarder` to the server's /api/v1/data/ingest.
//! The server should decode it through the ingester pipeline and write a row into
//! `flow_log.l7_flow_log`.
//!
//! Run (server must be the new build, listening on :30417):
//!     ZT_API_KEY=zt-test-key cargo run -p zerotrace-forwarder --example e2e_l7

use prost::Message;
use public::proto::flow_log::{AppProtoHead, AppProtoLogsBaseInfo, AppProtoLogsData};
use std::time::{SystemTime, UNIX_EPOCH};
use zerotrace_forwarder::Forwarder;

const MSG_PROTOCOLLOG: u8 = 5; // datatype.MESSAGE_TYPE_PROTOCOLLOG
const MESSAGE_HEADER_LEN: usize = 5;
const FLOW_HEADER_LEN: usize = 14;
const LATEST_VERSION: u16 = 0x8000;

/// FlowHeader (14B, little-endian): version=0x8000, encoder=RAW, team/org/agent ids.
fn flow_header(team_id: u32, org_id: u16, agent_id: u16) -> Vec<u8> {
    let mut h = vec![0u8; FLOW_HEADER_LEN];
    h[0..2].copy_from_slice(&LATEST_VERSION.to_le_bytes());
    h[2] = 0; // MESSAGE_ENCODER_RAW
    h[3..7].copy_from_slice(&team_id.to_le_bytes());
    h[7..9].copy_from_slice(&org_id.to_le_bytes());
    h[11..13].copy_from_slice(&agent_id.to_le_bytes());
    h
}

/// One wire frame: BaseHeader(5B: FrameSize BE u32 + Type u8) + FlowHeader + payload.
fn frame(msg_type: u8, payload: &[u8]) -> Vec<u8> {
    let fh = flow_header(0, 1, 12345);
    let frame_size = (MESSAGE_HEADER_LEN + fh.len() + payload.len()) as u32;
    let mut f = Vec::with_capacity(frame_size as usize);
    f.extend_from_slice(&frame_size.to_be_bytes());
    f.push(msg_type);
    f.extend_from_slice(&fh);
    f.extend_from_slice(payload);
    f
}

#[tokio::main]
async fn main() {
    // l7 base start/end times are nanoseconds since epoch (the writer derives the
    // row `time` column from them).
    let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;

    let data = AppProtoLogsData {
        base: Some(AppProtoLogsBaseInfo {
            start_time: now_ns,
            end_time: now_ns,
            flow_id: 1234567890,
            vtap_id: 12345,
            tap_type: 3,
            ip_src: u32::from_be_bytes([10, 0, 0, 1]),
            ip_dst: u32::from_be_bytes([10, 0, 0, 2]),
            port_src: 40000,
            port_dst: 80,
            protocol: 6, // TCP
            head: Some(AppProtoHead {
                proto: 20, // HTTP
                msg_type: 1,
                rrt: 100,
            }),
            ..Default::default()
        }),
        req_len: 123,
        resp_len: 456,
        version: "1.1".to_string(),
        ..Default::default()
    };

    let payload = data.encode_to_vec();
    // Per-message framing (libs/codec SimpleEncoder.WritePB): u32-LE length + proto.
    let mut framed = Vec::with_capacity(4 + payload.len());
    framed.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    framed.extend_from_slice(&payload);
    let wire = frame(MSG_PROTOCOLLOG, &framed);

    let fwd = Forwarder::builder()
        .base_url("http://127.0.0.1:30417")
        .api_key_from_env_or("zt-test-key")
        .agent_id("12345")
        .build()
        .expect("build forwarder");

    println!(
        "sending 1 l7 frame: {} payload bytes, {} total wire bytes, flow_id=1234567890",
        payload.len(),
        wire.len()
    );
    match fwd.upload_frames(wire).await {
        Ok(()) => println!("upload OK (server accepted the frame)"),
        Err(e) => println!("upload FAILED: {e}"),
    }
}
