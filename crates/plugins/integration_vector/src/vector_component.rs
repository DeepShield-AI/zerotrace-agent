use public::utils::net::IpMacPair;
use std::{sync::Arc, thread::JoinHandle};
use tokio::{runtime::Runtime, sync::broadcast::Sender};

pub struct VectorComponent {}

impl VectorComponent {
    pub fn new(
        _: bool,
        _: serde_yaml::Value,
        _: Arc<Runtime>,
        _: String,
        _: Arc<Sender<IpMacPair>>,
    ) -> Self {
        Self {}
    }

    pub fn start(&mut self) {}

    pub fn notify_stop(&mut self) -> Option<JoinHandle<()>> {
        None
    }

    pub fn on_config_change(&mut self, _: bool, _: serde_yaml::Value) {}
}
