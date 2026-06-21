// ── StackSample — profiling data ─────────────────────────────────────

use super::{SignalType, kind::SignalKind};
use smallvec::SmallVec;
use std::borrow::Cow;

#[derive(Debug, Clone, PartialEq)]
pub struct StackSample {
    pub process_name: Cow<'static, str>,
    pub pid: u32,
    pub thread_id: Option<u64>,
    pub stack_frames: SmallVec<[u64; 32]>,
    pub count: u64,
    pub timestamp_ns: i64,
}

impl StackSample {
    pub fn new(process_name: impl Into<Cow<'static, str>>, pid: u32, ts_ns: i64) -> Self {
        Self {
            process_name: process_name.into(),
            pid,
            thread_id: None,
            stack_frames: SmallVec::new(),
            count: 1,
            timestamp_ns: ts_ns,
        }
    }
    pub fn with_thread(mut self, tid: u64) -> Self {
        self.thread_id = Some(tid);
        self
    }
    pub fn with_frames(mut self, frames: &[u64]) -> Self {
        self.stack_frames = frames.iter().copied().collect();
        self
    }
    pub fn with_count(mut self, cnt: u64) -> Self {
        self.count = cnt;
        self
    }
}

impl SignalType for StackSample {
    fn signal_kind() -> SignalKind {
        SignalKind::PROFILE
    }
}
