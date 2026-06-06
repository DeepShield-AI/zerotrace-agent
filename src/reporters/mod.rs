//! Reporters — components that ship the agent's signals/data off-box.
//!
//! Groundwork for the new architecture (see `draft.md` L2 Reporters). For now this
//! only hosts the HTTP reporter that wraps `zerotrace-forwarder`; once the kernel /
//! runtime land (M0/M1), reporters become async `Reporter` components driven by the
//! pipeline and this `block_on` bridge goes away.

pub mod http;
