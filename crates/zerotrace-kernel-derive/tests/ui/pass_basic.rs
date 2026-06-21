//! This file tests that the derive compiles successfully.
//! Use `trybuild` to verify expected output.

use zerotrace_kernel::bundle::Bundle as _;
use zerotrace_kernel_derive::Bundle;
use std::sync::Arc;
use parking_lot::RwLock;

#[derive(Debug)]
struct DbPool;
#[derive(Debug)]
struct AppService;
#[derive(Debug)]
struct AuthService;
#[derive(Debug)]
struct OptionalFeature;

#[derive(Bundle)]
#[bundle(id = "test_bundle", name = "Test Bundle")]
struct TestBundle {
    #[component(id = "db", deps = [])]
    db: Arc<RwLock<DbPool>>,
    #[component(id = "svc", deps = [DbPool])]
    svc: Arc<RwLock<AppService>>,
    #[component(id = "opt", deps = [AuthService], optional)]
    opt: Arc<RwLock<OptionalFeature>>,
}

#[derive(Bundle)]
#[bundle(id = "req_bundle", name = "Required Bundle", required)]
struct RequiredBundle {
    #[component(id = "core", deps = [])]
    core: Arc<RwLock<DbPool>>,
}

#[derive(Bundle)]
#[bundle(id = "pipe_bundle")]
#[bundle(pipeline(
    name = "main",
    sources = ["cpu", "mem"],
    processors = ["tag"],
    reporters = ["http"]
))]
struct PipelineBundle {
    #[component(id = "src", deps = [])]
    src: Arc<RwLock<DbPool>>,
}

fn main() {
    // Verify the traits are implemented
    let bundle = TestBundle {
        db: Arc::new(RwLock::new(DbPool)),
        svc: Arc::new(RwLock::new(AppService)),
        opt: Arc::new(RwLock::new(OptionalFeature)),
    };
    let _components = bundle.components();
    let _id = bundle.id();
    let _name = bundle.name();
    assert!(!bundle.required());

    let req = RequiredBundle {
        core: Arc::new(RwLock::new(DbPool)),
    };
    assert!(req.required());

    let pipe = PipelineBundle {
        src: Arc::new(RwLock::new(DbPool)),
    };
    let pipelines = pipe.default_pipelines();
    assert_eq!(pipelines.len(), 1);
    assert_eq!(pipelines[0].name, "main");
    assert_eq!(pipelines[0].sources, vec!["cpu", "mem"]);
}
