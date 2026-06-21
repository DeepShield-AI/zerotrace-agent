// PipelineBlueprint: composes sources, processors, and reporters
// by name, resolving PipelineSpec references at build time.
//
// This bridges the gap between the DI container (World) and the
// PipelineExecutor — components are registered by name with automatic
// type erasure via [`BoxedSource`], [`BoxedProcessor`], and
// [`BoxedReporter`].
//
// # Registration modes
//
// | Method | Semantics | Use case |
// |--------|-----------|----------|
// | `add_source(S)` | Consuming (one-shot) | Unique collector per pipeline |
// | `add_source_boxed(BoxedSource)` | Consuming, pre-erased | SharedBlueprint materialization |
// | `add_reporter_shared(R: Clone)` | Non-consuming, auto-clone | Same HTTP forwarder in N pipelines |
//
// # Example — shared reporter across two pipelines
//
// ```
// use zerotrace_runtime::blueprint::PipelineBlueprint;
// use zerotrace_runtime::pipeline::{
//     CollectingReporter, IterSource, PipelineSpec,
// };
//
// let mut bp = PipelineBlueprint::new();
// bp.add_source("cpu", IterSource::new("cpu", vec![]));
// bp.add_source("mem", IterSource::new("mem", vec![]));
//
// // Register once — auto-cloned for each pipeline that references "http"
// bp.add_reporter_shared("http", CollectingReporter::new("http"));
//
// let h1 = bp.spawn(&PipelineSpec {
//     name: "metrics".into(),
//     source_ids: vec!["cpu".into()],
//     reporter_ids: vec!["http".into()],
//     ..Default::default()
// }).unwrap();
// let h2 = bp.spawn(&PipelineSpec {
//     name: "l7".into(),
//     source_ids: vec!["mem".into()],
//     reporter_ids: vec!["http".into()],  // same ref, auto-cloned
//     ..Default::default()
// }).unwrap();
// ```

use crate::pipeline::{
    BoxedProcessor, BoxedReporter, BoxedSource, PipelineExecutor, PipelineHandle, PipelineSpec,
};
use std::collections::HashMap;
use zerotrace_core::error::{Error, Result};

type SrcFactory = Box<dyn FnMut() -> BoxedSource + Send>;
type PrcFactory = Box<dyn FnMut() -> BoxedProcessor + Send>;
type RepFactory = Box<dyn FnMut() -> BoxedReporter + Send>;

/// Collects named sources, processors, and reporters, then resolves
/// a [`PipelineSpec`] into a running pipeline via [`PipelineExecutor::spawn`].
///
/// Components are type-erased on registration (via [`From`]/[`Into`]),
/// so different concrete types can coexist in the same blueprint.
///
/// # Two registration modes
///
/// **Consuming** (`add_*`, `add_*_boxed`): each component is consumed
/// on first spawn.  Use for unique sources.
///
/// **Shared** (`add_*_shared`): the template is cloned on each spawn.
/// Use for stateless processors/reporters shared across pipelines.
pub struct PipelineBlueprint {
    sources: HashMap<String, BoxedSource>,
    processors: HashMap<String, BoxedProcessor>,
    reporters: HashMap<String, BoxedReporter>,

    /// Shared factories — materialized on each spawn with unique names.
    shared_sources: HashMap<String, SrcFactory>,
    shared_processors: HashMap<String, PrcFactory>,
    shared_reporters: HashMap<String, RepFactory>,
    /// Monotonic counter for generating unique names for materialized clones.
    clone_counter: u64,
}

impl PipelineBlueprint {
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            processors: HashMap::new(),
            reporters: HashMap::new(),
            shared_sources: HashMap::new(),
            shared_processors: HashMap::new(),
            shared_reporters: HashMap::new(),
            clone_counter: 0,
        }
    }

    // ── Consuming registration ────────────────────────────────────────

    /// Register a named source.  The concrete type is erased immediately.
    pub fn add_source<S: crate::pipeline::Source + 'static>(
        &mut self,
        id: impl Into<String>,
        source: S,
    ) -> &mut Self {
        let id = id.into();
        if self.sources.contains_key(&id) {
            tracing::warn!("PipelineBlueprint: source [{id}] already registered — replacing");
        }
        self.sources.insert(id, source.into());
        self
    }

    /// Register a named processor.
    pub fn add_processor<P: crate::pipeline::Processor + 'static>(
        &mut self,
        id: impl Into<String>,
        processor: P,
    ) -> &mut Self {
        let id = id.into();
        if self.processors.contains_key(&id) {
            tracing::warn!("PipelineBlueprint: processor [{id}] already registered — replacing");
        }
        self.processors.insert(id, processor.into());
        self
    }

    /// Register a named reporter.
    pub fn add_reporter<R: crate::pipeline::Reporter + 'static>(
        &mut self,
        id: impl Into<String>,
        reporter: R,
    ) -> &mut Self {
        let id = id.into();
        if self.reporters.contains_key(&id) {
            tracing::warn!("PipelineBlueprint: reporter [{id}] already registered — replacing");
        }
        self.reporters.insert(id, reporter.into());
        self
    }

    // ── Boxed registration (pre-erased, for use by SharedBlueprint) ───

    /// Register an already-erased source.  Consuming — removed on first spawn.
    pub fn add_source_boxed(&mut self, id: impl Into<String>, source: BoxedSource) {
        self.sources.insert(id.into(), source);
    }

    /// Register an already-erased processor.
    pub fn add_processor_boxed(&mut self, id: impl Into<String>, proc: BoxedProcessor) {
        self.processors.insert(id.into(), proc);
    }

    /// Register an already-erased reporter.
    pub fn add_reporter_boxed(&mut self, id: impl Into<String>, rep: BoxedReporter) {
        self.reporters.insert(id.into(), rep);
    }

    // ── Shared registration (clone-on-spawn, non-consuming) ───────────

    /// Register a shared reporter template.  Each `spawn()` that references
    /// this name gets a fresh clone.  The template is never consumed.
    ///
    /// # Example
    ///
    /// ```ignore
    /// # use zerotrace_runtime::blueprint::PipelineBlueprint;
    /// # use zerotrace_runtime::pipeline::{CollectingReporter, PipelineSpec};
    /// let mut bp = PipelineBlueprint::new();
    /// bp.add_reporter_shared("http", CollectingReporter::new("http"));
    ///
    /// // Two pipelines share "http" — each gets its own clone.
    /// bp.spawn(&PipelineSpec {
    ///     name: "p1".into(), reporter_ids: vec!["http".into()], ..Default::default()
    /// }).unwrap();
    /// bp.spawn(&PipelineSpec {
    ///     name: "p2".into(), reporter_ids: vec!["http".into()], ..Default::default()
    /// }).unwrap();
    /// ```
    pub fn add_reporter_shared<R: crate::pipeline::Reporter + Clone + 'static>(
        &mut self,
        id: impl Into<String>,
        template: R,
    ) -> &mut Self {
        let id = id.into();
        let factory: RepFactory = Box::new(move || BoxedReporter::from(template.clone()));
        self.shared_reporters.insert(id, factory);
        self
    }

    /// Register a shared processor template.
    pub fn add_processor_shared<P: crate::pipeline::Processor + Clone + 'static>(
        &mut self,
        id: impl Into<String>,
        template: P,
    ) -> &mut Self {
        let id = id.into();
        let factory: PrcFactory = Box::new(move || BoxedProcessor::from(template.clone()));
        self.shared_processors.insert(id, factory);
        self
    }

    /// Register a shared source template (rare — most sources should be consuming).
    pub fn add_source_shared<S: crate::pipeline::Source + Clone + 'static>(
        &mut self,
        id: impl Into<String>,
        template: S,
    ) -> &mut Self {
        let id = id.into();
        let factory: SrcFactory = Box::new(move || BoxedSource::from(template.clone()));
        self.shared_sources.insert(id, factory);
        self
    }

    /// Register a shared reporter with an **explicit factory function**.
    ///
    /// Unlike [`add_reporter_shared`], this allows per-pipeline configuration.
    /// The factory receives the component's name (for logging) and can create
    /// differently-configured instances each time.
    ///
    /// # Example — different endpoints per pipeline
    ///
    /// ```ignore
    /// // Shared template with configurable endpoint
    /// bp.add_reporter_factory("http_forwarder", || {
    ///     BoxedReporter::from(HttpReporter::new("https://default/api"))
    /// });
    /// ```
    ///
    /// To use YAML `config:` overrides, resolve the config at the app layer
    /// and register per-pipeline instances via [`add_reporter_boxed`] or
    /// a custom factory before calling [`spawn`].
    pub fn add_reporter_factory<F>(&mut self, id: impl Into<String>, factory: F) -> &mut Self
    where
        F: FnMut() -> BoxedReporter + Send + 'static,
    {
        self.shared_reporters.insert(id.into(), Box::new(factory));
        self
    }

    /// Register a shared processor with an explicit factory function.
    pub fn add_processor_factory<F>(&mut self, id: impl Into<String>, factory: F) -> &mut Self
    where
        F: FnMut() -> BoxedProcessor + Send + 'static,
    {
        self.shared_processors.insert(id.into(), Box::new(factory));
        self
    }

    /// Number of consuming sources.
    pub fn source_count(&self) -> usize {
        self.sources.len()
    }
    /// Number of consuming processors.
    pub fn processor_count(&self) -> usize {
        self.processors.len()
    }
    /// Number of consuming reporters.
    pub fn reporter_count(&self) -> usize {
        self.reporters.len()
    }
    /// Number of shared reporter templates.
    pub fn shared_reporter_count(&self) -> usize {
        self.shared_reporters.len()
    }

    /// Check whether a source with the given id is available (consuming or shared).
    pub fn has_source(&self, id: &str) -> bool {
        self.sources.contains_key(id) || self.shared_sources.contains_key(id)
    }
    /// Check whether a processor with the given id is available.
    pub fn has_processor(&self, id: &str) -> bool {
        self.processors.contains_key(id) || self.shared_processors.contains_key(id)
    }
    /// Check whether a reporter with the given id is available.
    pub fn has_reporter(&self, id: &str) -> bool {
        self.reporters.contains_key(id) || self.shared_reporters.contains_key(id)
    }

    /// Resolve a [`PipelineSpec`] against registered components and spawn
    /// the pipeline.
    ///
    /// # Consuming behaviour
    ///
    /// Consuming components (registered via `add_*`) are **removed**.
    /// Shared components (registered via `add_*_shared`) are **cloned**
    /// on each spawn and remain available for future pipelines.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Pipeline`] if any referenced component is not
    /// found in either consuming or shared registries.
    pub fn spawn(&mut self, spec: &PipelineSpec) -> Result<PipelineHandle> {
        // Materialize shared factories into consuming maps before resolving.
        // We clone the ids into mutable vectors so materialize_shared can rewrite them.
        let mut source_ids: Vec<String> = spec.source_ids.clone();
        let mut processor_ids: Vec<String> = spec.processor_ids.clone();
        let mut reporter_ids: Vec<String> = spec.reporter_ids.clone();
        self.materialize_shared(&mut source_ids, &mut processor_ids, &mut reporter_ids);

        // Resolve sources (use materialized ids)
        let source_ids: Vec<String> = if source_ids.is_empty() {
            self.sources.keys().cloned().collect()
        } else {
            spec.source_ids.clone()
        };

        let mut sources: Vec<(String, BoxedSource)> = Vec::with_capacity(source_ids.len());
        for id in &source_ids {
            let source = self.sources.remove(id).ok_or_else(|| Error::Pipeline {
                message: format!(
                    "source [{id}] not registered in blueprint (already consumed or missing)"
                ),
                fatal: true,
            })?;
            sources.push((id.clone(), source));
        }

        // Resolve processors
        let processor_ids: Vec<String> = if processor_ids.is_empty() {
            self.processors.keys().cloned().collect()
        } else {
            processor_ids
        };

        let mut processors: Vec<(String, BoxedProcessor)> = Vec::with_capacity(processor_ids.len());
        for id in &processor_ids {
            let processor = self.processors.remove(id).ok_or_else(|| Error::Pipeline {
                message: format!(
                    "processor [{id}] not registered in blueprint (already consumed or missing)"
                ),
                fatal: true,
            })?;
            processors.push((id.clone(), processor));
        }

        // Resolve reporters
        let reporter_ids: Vec<String> = if reporter_ids.is_empty() {
            self.reporters.keys().cloned().collect()
        } else {
            reporter_ids
        };

        let mut reporters: Vec<(String, BoxedReporter)> = Vec::with_capacity(reporter_ids.len());
        for id in &reporter_ids {
            let reporter = self.reporters.remove(id).ok_or_else(|| Error::Pipeline {
                message: format!(
                    "reporter [{id}] not registered in blueprint (already consumed or missing)"
                ),
                fatal: true,
            })?;
            reporters.push((id.clone(), reporter));
        }

        Ok(PipelineExecutor::spawn(
            spec, sources, processors, reporters,
        ))
    }

    /// Before resolving, clone shared templates into the consuming maps
    /// under unique internal names, and rewrite the spec ids in place.
    fn materialize_shared(
        &mut self,
        source_ids: &mut Vec<String>,
        processor_ids: &mut Vec<String>,
        reporter_ids: &mut Vec<String>,
    ) {
        // Collect materialized clones into temporary Vecs first to avoid
        // simultaneous borrow of self.shared_* and self.{sources,processors,reporters}.
        let mut mat_sources: Vec<(String, BoxedSource)> = Vec::new();
        let mut mat_processors: Vec<(String, BoxedProcessor)> = Vec::new();
        let mut mat_reporters: Vec<(String, BoxedReporter)> = Vec::new();
        let mut rewrites: Vec<(String, String)> = Vec::new();

        for id in source_ids.iter() {
            if !self.sources.contains_key(id.as_str()) {
                if self.shared_sources.contains_key(id.as_str()) {
                    let unique = self.next_unique(id);
                    if let Some(factory) = self.shared_sources.get_mut(id.as_str()) {
                        mat_sources.push((unique.clone(), factory()));
                        rewrites.push((id.clone(), unique));
                    }
                }
            }
        }
        for id in processor_ids.iter() {
            if !self.processors.contains_key(id.as_str()) {
                if self.shared_processors.contains_key(id.as_str()) {
                    let unique = self.next_unique(id);
                    if let Some(factory) = self.shared_processors.get_mut(id.as_str()) {
                        mat_processors.push((unique.clone(), factory()));
                        rewrites.push((id.clone(), unique));
                    }
                }
            }
        }
        for id in reporter_ids.iter() {
            if !self.reporters.contains_key(id.as_str()) {
                if self.shared_reporters.contains_key(id.as_str()) {
                    let unique = self.next_unique(id);
                    if let Some(factory) = self.shared_reporters.get_mut(id.as_str()) {
                        mat_reporters.push((unique.clone(), factory()));
                        rewrites.push((id.clone(), unique));
                    }
                }
            }
        }

        // Insert materialized clones (no borrow conflict now)
        for (unique, src) in mat_sources {
            self.sources.insert(unique, src);
        }
        for (unique, proc) in mat_processors {
            self.processors.insert(unique, proc);
        }
        for (unique, rep) in mat_reporters {
            self.reporters.insert(unique, rep);
        }

        // Rewrite spec ids to point to the materialized clones
        for (original, unique) in &rewrites {
            for id in source_ids.iter_mut() {
                if id == original {
                    *id = unique.clone();
                }
            }
            for id in processor_ids.iter_mut() {
                if id == original {
                    *id = unique.clone();
                }
            }
            for id in reporter_ids.iter_mut() {
                if id == original {
                    *id = unique.clone();
                }
            }
        }
    }

    fn next_unique(&mut self, base: &str) -> String {
        self.clone_counter += 1;
        format!("{base}__shared_{}", self.clone_counter)
    }
}

impl Default for PipelineBlueprint {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::{CollectingReporter, FnProcessor, IterSource};
    use std::{sync::Arc, time::Duration};
    use zerotrace_core::signal::{Batch, BatchMetadata, SignalKind};

    fn make_batch() -> Batch {
        Batch {
            items: vec![],
            metadata: Arc::new(BatchMetadata::new("test")),
        }
    }

    #[tokio::test]
    async fn test_blueprint_spawn_single_source() {
        let mut bp = PipelineBlueprint::new();
        bp.add_source("s1", IterSource::new("s1", vec![make_batch()]));
        bp.add_reporter("r1", CollectingReporter::new("r1"));

        let spec = PipelineSpec {
            name: "test".into(),
            source_ids: vec!["s1".into()],
            reporter_ids: vec!["r1".into()],
            ..Default::default()
        };

        let handle = bp.spawn(&spec).unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_blueprint_spawn_consumes_components() {
        let mut bp = PipelineBlueprint::new();
        bp.add_source("s1", IterSource::new("s1", vec![make_batch()]));
        bp.add_reporter("r1", CollectingReporter::new("r1"));

        assert!(bp.has_source("s1"));
        assert!(bp.has_reporter("r1"));

        let spec = PipelineSpec {
            name: "test".into(),
            source_ids: vec!["s1".into()],
            reporter_ids: vec!["r1".into()],
            ..Default::default()
        };

        let handle = bp.spawn(&spec).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        handle.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Components are consumed
        assert!(!bp.has_source("s1"));
        assert!(!bp.has_reporter("r1"));
        assert_eq!(bp.source_count(), 0);
        assert_eq!(bp.reporter_count(), 0);
    }

    #[tokio::test]
    async fn test_blueprint_double_spawn_error() {
        let mut bp = PipelineBlueprint::new();
        bp.add_source("s1", IterSource::new("s1", vec![]));
        bp.add_reporter("r1", CollectingReporter::new("r1"));

        let spec = PipelineSpec {
            name: "test".into(),
            source_ids: vec!["s1".into()],
            reporter_ids: vec!["r1".into()],
            ..Default::default()
        };

        // First spawn consumes components
        let _handle = bp.spawn(&spec).unwrap();

        // Second spawn should fail with a clear error
        let result = bp.spawn(&spec);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("already consumed") || err_msg.contains("not registered"),
            "expected 'already consumed' error, got: {err_msg}"
        );
    }

    #[test]
    fn test_blueprint_counters() {
        let mut bp = PipelineBlueprint::new();
        assert_eq!(bp.source_count(), 0);
        bp.add_source("a", IterSource::new("a", vec![]));
        bp.add_source("b", IterSource::new("b", vec![]));
        assert_eq!(bp.source_count(), 2);
        bp.add_processor(
            "p",
            FnProcessor::new("p", |_: &mut Batch| -> zerotrace_core::error::Result<()> {
                Ok(())
            }),
        );
        assert_eq!(bp.processor_count(), 1);
        bp.add_reporter("r", CollectingReporter::new("r"));
        assert_eq!(bp.reporter_count(), 1);
    }

    #[test]
    fn test_blueprint_missing_source_error() {
        let mut bp = PipelineBlueprint::new();
        let spec = PipelineSpec {
            name: "missing".into(),
            source_ids: vec!["nonexistent".into()],
            ..Default::default()
        };
        let result = bp.spawn(&spec);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("nonexistent"));
        assert!(msg.contains("source"));
    }

    #[test]
    fn test_blueprint_has_queries() {
        let mut bp = PipelineBlueprint::new();
        assert!(!bp.has_source("s1"));
        assert!(!bp.has_processor("p1"));
        assert!(!bp.has_reporter("r1"));

        bp.add_source("s1", IterSource::new("s1", vec![]));
        bp.add_processor(
            "p1",
            FnProcessor::new("p1", |_: &mut Batch| -> zerotrace_core::error::Result<()> {
                Ok(())
            }),
        );
        bp.add_reporter("r1", CollectingReporter::new("r1"));

        assert!(bp.has_source("s1"));
        assert!(bp.has_processor("p1"));
        assert!(bp.has_reporter("r1"));
    }

    // ── Shared reporter tests ─────────────────────────────────────────

    #[tokio::test]
    async fn test_shared_reporter_across_two_pipelines() {
        let mut bp = PipelineBlueprint::new();

        bp.add_source("cpu", IterSource::new("cpu", vec![make_batch()]));
        bp.add_source("mem", IterSource::new("mem", vec![make_batch()]));
        bp.add_reporter_shared("http", CollectingReporter::new("http"));

        // Two pipelines, same reporter ref — each gets its own clone
        let h1 = bp
            .spawn(&PipelineSpec {
                name: "p1".into(),
                source_ids: vec!["cpu".into()],
                reporter_ids: vec!["http".into()],
                ..Default::default()
            })
            .unwrap();
        let h2 = bp
            .spawn(&PipelineSpec {
                name: "p2".into(),
                source_ids: vec!["mem".into()],
                reporter_ids: vec!["http".into()],
                ..Default::default()
            })
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        h1.shutdown();
        h2.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // Shared reporter is NOT consumed — still available
        assert!(bp.has_reporter("http"));
        assert_eq!(bp.shared_reporter_count(), 1);
        assert_eq!(bp.reporter_count(), 0); // clones were consumed
    }

    #[tokio::test]
    async fn test_shared_reporter_can_spawn_three_times() {
        let mut bp = PipelineBlueprint::new();
        bp.add_source("s1", IterSource::new("s1", vec![make_batch()]));
        bp.add_source("s2", IterSource::new("s2", vec![make_batch()]));
        bp.add_source("s3", IterSource::new("s3", vec![make_batch()]));
        bp.add_reporter_shared("r", CollectingReporter::new("r"));

        let h1 = bp
            .spawn(&PipelineSpec {
                name: "p1".into(),
                source_ids: vec!["s1".into()],
                reporter_ids: vec!["r".into()],
                ..Default::default()
            })
            .unwrap();
        let h2 = bp
            .spawn(&PipelineSpec {
                name: "p2".into(),
                source_ids: vec!["s2".into()],
                reporter_ids: vec!["r".into()],
                ..Default::default()
            })
            .unwrap();
        let h3 = bp
            .spawn(&PipelineSpec {
                name: "p3".into(),
                source_ids: vec!["s3".into()],
                reporter_ids: vec!["r".into()],
                ..Default::default()
            })
            .unwrap();

        tokio::time::sleep(Duration::from_millis(100)).await;
        h1.shutdown();
        h2.shutdown();
        h3.shutdown();
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert!(bp.has_reporter("r"));
        assert_eq!(bp.shared_reporter_count(), 1);
    }

    #[test]
    fn test_shared_reporter_shows_in_has_query() {
        let mut bp = PipelineBlueprint::new();
        assert!(!bp.has_reporter("http"));
        bp.add_reporter_shared("http", CollectingReporter::new("http"));
        assert!(bp.has_reporter("http"));
        assert_eq!(bp.shared_reporter_count(), 1);
    }
}
