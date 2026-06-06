# ZeroTrace Agent / Server 架构设计（draft）

> **生成时间**：2026-05-28
> **基线分支**：`main @ 913e3642d`
> **范围**：本仓库（agent，Rust）+ `../zerotrace-server`（server，Go，fork 自 deepflowio/deepflow）
> **执行窗口**：4 个月（16 周）

## 0. 目标与非目标

### 0.1 设计目标

| # | 目标 | 检验 |
|---|---|---|
| G1 | **架构可扩展**：未来加 AI、安全、新协议、新信号类型，**不动核心代码** | 加新 Bundle = 新建一个 crate + 一份配置；改不到 `kernel` / `runtime` |
| G2 | **业务可配置**：业务能力切换走运行时配置，不走重新编译 | 同一二进制可启用/禁用任意采集模式；YAML 改后 `SIGHUP` 热加载 |
| G3 | **组件可测试**：每个组件可独立单测，无需启动整个 agent | 每个 Component trait 都有 `Mock` 实现；新组件覆盖率 ≥ 80% |
| G4 | **渐进迁移**：保留 DeepFlow 现有全部采集能力，重构与新功能开发可并行 | 试点组件迁完后，旧路径仍可跑；新旧组件可同时在 World 中存在 |
| G5 | **公有云形态**：agent → server 通信改 HTTP 短连接 + API_KEY；server 由我方运营 | agent 离线时本机仍可调试；server 端点全部带鉴权 |

### 0.2 非目标（本期不做）

- ❌ AI 处理器、AI 模型部署（仅留 trait 接缝）
- ❌ 安全采集（FIM、syscall audit、漏洞扫描；仅留 SignalKind 和 Bundle 占位）
- ❌ WASM 插件（P3 之后再考虑；P1–P2 只做 .so）
- ❌ 前端 Web 控制台（API 留好，前端独立项目）
- ❌ 数据面 hot path 重构（`dispatcher` / `flow_generator` / `ebpf_dispatcher` 内部实现保持不变，仅包成 Component）

---

## 1. 设计灵感来源与取舍

### 1.1 Bevy DI（来自 `todo.md`）

**强项**：
- `World` 类型擦除资源容器，新增组件无需修改容器结构
- `SystemParam` 让函数参数自动注入，与 `arc_swap::Access<T>` 天然兼容
- 渐进式落地——新旧组件可在同一 `AgentComponents` 内共存
- 纯 Rust，无 reflection 包袱

**弱项**：
- 无组件接口/实现分离的约定 → 测试 mock 要靠 trait object 临时套
- 无"组件分组"概念 → 大量组件难以按业务域组织
- 无 Lifecycle 钩子约定 → 仍需手写 start/stop

### 1.2 Datadog `comp/` 框架

**强项**：
- **接口/实现分离**：每个组件先定义 Go interface，再写 `impl/`、`noopimpl/`、`mockimpl/` 三套
- **Bundle 概念**：相关组件打包成 `fxutil.Bundle`，作为可选/必选的整体装载
- **fx.Lifecycle 钩子**：组件在构造函数里向 lifecycle 注册 `OnStart` / `OnStop`，无须任何 god-object 调度
- **依赖声明在构造函数签名**：依赖关系即代码、即文档
- **测试头等公民**：每个组件随附 mock，集成测试用 `fx.New(mockBundle)` 拼出最小可测系统

**弱项**：
- 基于 Go fx 反射 → Rust 没有 reflect，要靠 trait + `TypeId` 模拟
- 大量样板（每个组件 4 个目录：`def/`、`impl/`、`noopimpl/`、`mockimpl/`）
- 启动 DAG 解析在大项目里有性能影响（agent 启动一次，影响小）

### 1.3 取舍：取 Bevy 的 DI 机制 + Datadog 的工程规约

| 维度 | 选 Bevy | 选 Datadog | 我们的选择 |
|---|---|---|---|
| 资源容器 | World/TypeId | fx provider graph | **Bevy World**（Rust 自然） |
| 参数注入 | SystemParam | 构造函数签名 | **Bevy SystemParam**（与 arc_swap 兼容） |
| 接口分离 | 无约定 | def/impl/mock | **Datadog 风格**（每组件 trait + impl + mock） |
| 组件分组 | 无 | Bundle | **Datadog Bundle**（强约定） |
| 启停钩子 | 无 | fx.Lifecycle | **Datadog Lifecycle**（hook 注册式） |
| 调度 | Scheduler | fx 拓扑排序 | **混合**（启动期 fx 风格拓扑、运行期 Pipeline DAG） |

**核心成果**：**Component（Datadog 接口/实现分离）+ DI Kernel（Bevy World/SystemParam）+ Bundle（Datadog 分组）+ Pipeline（声明式 DSL）** 四层。

---

## 2. 总体架构：四层模型

```
┌───────────────────────────────────────────────────────────────────┐
│ L4  Pipeline (用户面 YAML)                                         │
│       sources: [...] → processors: [...] → reporters: [...]       │
│       每条 pipeline 一个 tokio task，组件间 mpsc 串接             │
├───────────────────────────────────────────────────────────────────┤
│ L3  Bundle (分组与可选边界)                                        │
│       MicroserviceBundle / HostMetricBundle / MirrorBundle ...    │
│       未来：AiBundle / SecurityBundle                              │
│       每个 Bundle 对应一个 crate，提供组件清单 + 配置 schema      │
├───────────────────────────────────────────────────────────────────┤
│ L2  Component (单元代码)                                           │
│       trait Component (def) + impl (real/noop/mock)               │
│       lifecycle hooks: on_start / on_stop / on_reload             │
│       依赖通过 SystemParam 注入                                    │
├───────────────────────────────────────────────────────────────────┤
│ L1  Kernel (DI 核心)                                               │
│       World (TypeId → Box<dyn Any+Send+Sync>)                     │
│       SystemParam (Res / ResMut / Cfg / Sender / Recv)            │
│       LifecycleRegistry (启停钩子)                                 │
│       ConfigBus (订阅式配置变更分发)                               │
└───────────────────────────────────────────────────────────────────┘
```

四层职责严格隔离：
- **L1 几乎不变**——抽象一次后稳定多年；
- **L2 是开发者主要工作面**——写新功能 = 加新 Component；
- **L3 是产品形态决策面**——同一个 binary 装哪些 bundle、用户可选 vs 必选；
- **L4 是运维面**——只动配置不动代码。

---

## 3. L1 Kernel：核心抽象

### 3.1 `World` 资源容器

```rust
// crates/zerotrace-kernel/src/world.rs
use std::any::{Any, TypeId};
use std::collections::HashMap;
use parking_lot::RwLock;
use std::sync::Arc;

pub struct World {
    resources: RwLock<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
}

impl World {
    pub fn new() -> Self { Self { resources: RwLock::new(HashMap::new()) } }

    /// 插入资源。若同类型已存在则替换（用于热加载）。
    pub fn insert<T: Any + Send + Sync>(&self, value: T) {
        self.resources.write().insert(TypeId::of::<T>(), Arc::new(value));
    }

    /// 读取资源句柄。返回 Arc 而非引用，避免锁生命周期蔓延。
    pub fn get<T: Any + Send + Sync>(&self) -> Option<Arc<T>> {
        let map = self.resources.read();
        let any = map.get(&TypeId::of::<T>())?.clone();
        // SAFETY: TypeId 已比对
        let raw = Arc::into_raw(any) as *const T;
        Some(unsafe { Arc::from_raw(raw) })
    }

    pub fn contains<T: Any + Send + Sync>(&self) -> bool {
        self.resources.read().contains_key(&TypeId::of::<T>())
    }
}
```

**关键决策**：
- 用 `parking_lot::RwLock` 而非 `std::sync::RwLock`（性能 + 无 poison）。
- 资源以 `Arc<T>` 持有，**取出后释放写锁**——避免 hot path 持锁。
- 不实现 Bevy 教程那种"同一 system 内 `Res<T>` 与 `ResMut<T>` 互斥"的运行时检查（agent 用法简单不需要）。
- 不做编译期借用检查（Bevy ECS 的高级特性），用 `Arc<RwLock<T>>` / `Arc<Mutex<T>>` 显式表达内部可变。

### 3.2 `SystemParam` 自动注入

```rust
// crates/zerotrace-kernel/src/param.rs
pub trait SystemParam: Sized {
    type Item<'w>;
    fn fetch<'w>(world: &'w World) -> Result<Self::Item<'w>, KernelError>;
}

/// 共享只读资源（最常用，等同 Arc<T>）
pub struct Res<T: Any + Send + Sync>(Arc<T>);
impl<T: Any + Send + Sync> Deref for Res<T> { ... }

/// 配置句柄（基于 arc_swap，hot-reload 友好）
pub struct Cfg<T: Any + Send + Sync>(arc_swap::access::DynAccess<Arc<T>>);
impl<T: Any + Send + Sync> Cfg<T> {
    pub fn load(&self) -> Arc<T> { self.0.load() }
}

/// mpsc 发送端（pipeline 内部串接）
pub struct Sender<T: Send + 'static>(tokio::sync::mpsc::Sender<T>);

/// mpsc 接收端
pub struct Recv<T: Send + 'static>(tokio::sync::mpsc::Receiver<T>);

// 1..=12 元 tuple 通过宏一次性 impl
impl<P1: SystemParam, P2: SystemParam> SystemParam for (P1, P2) { ... }
```

为什么这样设计：
- **`Res<T>` 持 `Arc<T>` 而非 `&T`**——可以在 async task 间随意传递、跨 await 持有；
- **`Cfg<T>` 走 `arc_swap`**——与 DeepFlow 现有 `ConfigHandler` 的 access 哲学一致，迁移成本低；
- **`Sender<T>` / `Recv<T>` 是 mpsc 包装**——Pipeline 拼装时由 Executor 创建并注入。

### 3.3 `Lifecycle` 钩子

```rust
// crates/zerotrace-kernel/src/lifecycle.rs
#[async_trait::async_trait]
pub trait Lifecycle: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    /// 启动钩子。框架按拓扑顺序调用。
    async fn on_start(&mut self, ctx: &LifecycleCtx) -> Result<(), KernelError> { Ok(()) }

    /// 停止钩子。按启动逆序调用。
    async fn on_stop(&mut self, ctx: &LifecycleCtx) -> Result<(), KernelError> { Ok(()) }

    /// 配置热加载。收到 ConfigBus 事件后框架调用。
    async fn on_reload(&mut self, ctx: &LifecycleCtx, change: &ConfigChange)
        -> Result<(), KernelError> { Ok(()) }

    /// 健康检查，给 server 的 heartbeat 用。
    fn health(&self) -> Health { Health::Healthy }
}

pub enum Health {
    Healthy,
    Degraded { reason: String },
    Down     { reason: String },
}

pub struct LifecycleCtx<'a> {
    pub world: &'a World,
    pub runtime: &'a tokio::runtime::Handle,
}

/// 全局钩子注册器
pub struct LifecycleRegistry {
    hooks: Vec<Box<dyn Lifecycle>>,
}

impl LifecycleRegistry {
    pub fn register<L: Lifecycle>(&mut self, hook: L) -> ComponentId;
    pub async fn start_all(&mut self, ctx: &LifecycleCtx) -> Result<()>;
    pub async fn stop_all(&mut self, ctx: &LifecycleCtx) -> Result<()>;   // 逆序
}
```

**与 Datadog `fx.Lifecycle` 对齐**：组件在自己的构造函数里 `registry.register(self.clone())` 注册，**不再由 god-object 决定启停顺序**。

### 3.4 `ConfigBus` 订阅式配置变更分发

```rust
// crates/zerotrace-kernel/src/config_bus.rs
pub enum ConfigChange {
    /// 通用：某个 typed config 整体替换
    Replaced { type_id: TypeId, old: Arc<dyn Any+Send+Sync>, new: Arc<dyn Any+Send+Sync> },
    /// 细粒度变更（subscriber 可按需 match）
    Field { path: ConfigPath, old: serde_json::Value, new: serde_json::Value },
}

pub trait ConfigSubscriber: Send + Sync {
    fn interested(&self, change: &ConfigChange) -> bool;
    async fn on_change(&mut self, change: &ConfigChange, ctx: &LifecycleCtx) -> Result<Action>;
}

pub enum Action {
    None,
    HotApplied,
    RestartSelf,
    RestartPipeline(&'static str),
    RestartAgent,
}

pub struct ConfigBus {
    subscribers: Vec<Box<dyn ConfigSubscriber>>,
}
```

替代 DeepFlow 现状中 `ConfigHandler::on_config` 的 435 个 if-else（见 `todo.md §2.2`）。每个 subscriber 独立实现、独立测试。

---

## 4. L2 Component：单元代码

### 4.1 Component 接口/实现分离（Datadog 规约）

每个组件按以下目录结构组织：

```
crates/zerotrace-source-proc/
├── Cargo.toml
└── src/
    ├── lib.rs                  # 仅 re-export + 注册 Bundle
    ├── cpu/
    │   ├── mod.rs              # trait CpuCollector  ← 接口（def）
    │   ├── real.rs             # impl from /proc/stat ← 真实实现
    │   ├── noop.rs             # noop 实现（用于禁用场景）
    │   └── mock.rs             # mock 实现（feature = "test-utils"）
    ├── memory/
    │   ├── mod.rs
    │   ├── real.rs
    │   ├── noop.rs
    │   └── mock.rs
    └── ...
```

```rust
// crates/zerotrace-source-proc/src/cpu/mod.rs
#[async_trait]
pub trait CpuCollector: Source<Output = MetricBatch> + Lifecycle {
    fn cpu_count(&self) -> usize;
}

pub use real::RealCpuCollector;
pub use noop::NoopCpuCollector;
#[cfg(any(test, feature = "test-utils"))]
pub use mock::MockCpuCollector;
```

```rust
// crates/zerotrace-source-proc/src/cpu/real.rs
pub struct RealCpuCollector {
    interval: Duration,
    cpu_count: usize,
}

impl RealCpuCollector {
    /// 构造函数即依赖声明：参数列表 = 依赖。
    /// 等价于 Datadog 的 New(deps...) Component。
    pub fn new(cfg: Cfg<CpuCollectorConfig>, registry: &mut LifecycleRegistry) -> Arc<Self> {
        let me = Arc::new(Self {
            interval: cfg.load().interval,
            cpu_count: num_cpus::get(),
        });
        registry.register(me.clone()); // 自注册 lifecycle
        me
    }
}

#[async_trait]
impl Lifecycle for RealCpuCollector {
    fn name(&self) -> &'static str { "source.proc.cpu" }
    async fn on_start(&mut self, ctx: &LifecycleCtx) -> Result<()> { ... }
    async fn on_stop (&mut self, ctx: &LifecycleCtx) -> Result<()> { ... }
}

#[async_trait]
impl Source for RealCpuCollector {
    type Output = MetricBatch;
    fn signals(&self) -> &[SignalKind] { &[SignalKind::Metric] }
    async fn run(&mut self, mut sink: SignalSink<MetricBatch>) -> Result<()> { ... }
}

impl CpuCollector for RealCpuCollector {
    fn cpu_count(&self) -> usize { self.cpu_count }
}
```

**规约总结**：
| 文件 | 强制 | 用途 |
|---|---|---|
| `mod.rs` | ✓ | 仅 trait + re-export |
| `real.rs` | ✓ | 真实实现，依赖外部资源 |
| `noop.rs` | ✓ | 空实现，配置禁用时使用，不依赖任何外部资源 |
| `mock.rs` | ✓ (feature-gated) | 测试用 mock，可控行为 |

### 4.2 信号类型（Signal）

```rust
// crates/zerotrace-core/src/signal.rs
pub enum Signal {
    Metric  (MetricPoint),
    Trace   (Span),
    Log     (LogRecord),
    Profile (StackSample),
    Event   (SystemEvent),
    /// 未来扩展（AI 推断结果、安全事件）通过 Custom 注入，无需改枚举
    Custom  (Arc<dyn ErasedSignal>),
}

pub trait ErasedSignal: Any + Send + Sync + Debug {
    fn kind_name(&self) -> &'static str;
    fn as_any(&self) -> &dyn Any;
}

pub enum SignalKind {
    Metric, Trace, Log, Profile, Event,
    Custom(&'static str),  // 例如 "anomaly", "security.fim"
}

pub struct SignalBatch {
    pub kind: SignalKind,
    pub items: Vec<Signal>,
    pub deadline: Option<Instant>,  // pipeline 内有时限传递
}
```

**Custom 变体的意义**：未来 AI / 安全模块加新信号类型时，**不修改 `Signal` 枚举**——只需定义新 struct + 实现 `ErasedSignal`，通过 `Signal::Custom(Arc::new(MyAnomaly { ... }))` 注入。下游 processor 用 `as_any().downcast_ref::<MyAnomaly>()` 拿回原类型。

### 4.3 三大组件 trait

```rust
#[async_trait]
pub trait Source: Lifecycle {
    type Output: Into<Signal> + Send + 'static;
    fn signals(&self) -> &[SignalKind];
    fn capabilities(&self) -> SourceCaps;   // needs_root / needs_ebpf / linux_only
    async fn run(&mut self, sink: SignalSink<Self::Output>) -> Result<()>;
}

#[async_trait]
pub trait Processor: Lifecycle {
    fn accepts(&self) -> &[SignalKind];
    fn produces(&self) -> &[SignalKind];
    async fn process(&mut self, batch: SignalBatch) -> Result<SignalBatch>;
}

#[async_trait]
pub trait Reporter: Lifecycle {
    fn accepts(&self) -> &[SignalKind];
    async fn submit(&mut self, batch: SignalBatch) -> Result<()>;
}
```

启动期 PipelineExecutor 用 `accepts()` / `produces()` 做**静态类型校验**：若 A → B → C 中 B 不接受 A 的输出，启动失败而非运行期崩。

---

## 5. L3 Bundle：分组与可选边界

### 5.1 Bundle trait

```rust
// crates/zerotrace-kernel/src/bundle.rs
pub trait Bundle: Send + Sync + 'static {
    /// Bundle 全局唯一 ID
    fn id() -> &'static str where Self: Sized;
    /// 人类可读名称
    fn name() -> &'static str where Self: Sized;
    /// 该 Bundle 提供的组件描述
    fn components(&self) -> Vec<ComponentDescriptor>;
    /// 可选：该 Bundle 推荐的默认 pipeline 模板
    fn default_pipelines(&self) -> Vec<PipelineTemplate> { Vec::new() }
    /// 该 Bundle 配置 schema
    fn config_schema(&self) -> Schema;
    /// 该 Bundle 是否必选（核心 bundle 设 true）
    fn required(&self) -> bool { false }
}

pub struct ComponentDescriptor {
    pub id: &'static str,
    pub factory: Box<dyn Fn(&World, &mut LifecycleRegistry) -> Result<Arc<dyn Any+Send+Sync>>>,
    pub provides: TypeId,         // 该组件产出什么类型 → 注册到 World 用
    pub deps:     Vec<TypeId>,    // 依赖哪些 World 资源
    pub optional: bool,           // 缺依赖时是否降级
}
```

### 5.2 既定 Bundle 清单

按 PPT 业务划分（不实现 AI/安全，但留位）：

| Bundle | crate | 默认 | 包含组件 |
|---|---|---|---|
| `core` | `zerotrace-bundle-core` | ✓ 必选 | World、ConfigBus、LifecycleRegistry、ApiKey、Time、HealthReporter |
| `host-metric` | `zerotrace-bundle-host-metric` | ✓ | CpuCollector / MemCollector / DiskCollector / NetCollector |
| `microservice` | `zerotrace-bundle-microservice` | ✓ | EbpfSyscallSource、EbpfTlsSource (OpenSSL/Go)、FlowAggregateProcessor、L7ParseProcessor、ReorderProcessor、ReassemblyProcessor、SqlObfuscateProcessor、TraceAssembleProcessor、TaggingProcessor、HttpReporter |
| `packet-capture` | `zerotrace-bundle-packet-capture` | ✗ | LocalAfPacketSource、LocalMultiNsSource |
| `mirror` | `zerotrace-bundle-mirror` | ✗ | MirrorSource、AnalyzerSource、DecapsulateProcessor |
| `npb` | `zerotrace-bundle-npb` | ✗ | NpbForwardReporter、NpbBandwidthExtension |
| `cloud-platform` | `zerotrace-bundle-cloud-platform` | ✗ | ImdsTagger、K8sWatcher、LibvirtTagger |
| `integration` | `zerotrace-bundle-integration` | ✗ | OtelReceiver、PrometheusReceiver、DatadogReceiver、TelegrafReceiver、SkyWalkingReceiver |
| `debug` | `zerotrace-bundle-debug` | ✓ | DebugSocketServer、StatsReporter |
| `ai` | `zerotrace-bundle-ai` *(未来)* | ✗ | *预留占位 crate，本期只有 Cargo.toml + 空 lib.rs* |
| `security` | `zerotrace-bundle-security` *(未来)* | ✗ | *同上* |

**装载逻辑**：
- 必选 bundle 由 `bin/zerotrace-agent/src/main.rs` 硬编码装；
- 可选 bundle 在 YAML 顶层 `bundles: [...]` 声明启用，启动期由 BundleLoader 注册到 World。
- AI / 安全 bundle 留空 crate（仅 Cargo.toml + `pub fn register() {}`），有真实需求时再填实现。

### 5.3 与 Cargo Features 的关系

**严格规则**：
- **Cargo feature 只能由"外部依赖 / 平台限制"驱动**，例如 `linux-ebpf`（依赖 aya + clang 编译 shim）、`dpdk`（依赖 DPDK lib）、`dynamic-plugin`（依赖 libloading）；
- **不允许**用 feature 切业务能力（不要有 `--features microservice`）；
- 业务能力的"装不装"通过 Bundle 控制（编译期是否引用某 crate）；
- 业务能力的"开不开"通过 YAML pipeline 控制（运行时配置）。

Cargo features 总表：

| feature | 默认 | 控制 |
|---|---|---|
| `linux-ebpf` | linux 默认 ✓ | `zerotrace-source-ebpf` 编译 |
| `linux-af-packet` | linux 默认 ✓ | `zerotrace-source-packet` |
| `windows-pktmon` | win 默认 ✓ | `source-packet` win 实现 |
| `dpdk` | ✗ | DPDK 路径 |
| `dynamic-plugin` | unix 默认 ✓ | .so 插件加载 |
| `test-utils` | ✗ | mock 实现导出 |
| `enterprise` | ✗ | `crates/enterprise-utils` |

---

## 6. L4 Pipeline：声明式 DSL

### 6.1 YAML 语法

```yaml
# /etc/zerotrace-agent.yaml

# ── L3：装载哪些 bundle ──────────────────────────
bundles:
  - core                  # 必选，可省略
  - host-metric
  - microservice
  - cloud-platform        # 可选
  # - mirror              # 可选，禁用
  # - npb
  # - integration

# ── L4：pipeline 声明 ───────────────────────────
pipelines:
  microservice_trace:
    enabled: true
    sources:
      - { ref: ebpf_syscall, config: { protocols: [http1, http2, grpc, mysql, redis, dns] } }
      - { ref: ebpf_tls_openssl }
      - { ref: ebpf_tls_go }
    processors:
      - { ref: reorder,         config: { buffer_ms: 200 } }
      - { ref: reassembly,      config: { timeout_s: 30 } }
      - { ref: l7_parse }
      - { ref: tagging,         config: { sources: [imds, k8s] } }
      - { ref: trace_assemble }
    reporters:
      - { ref: http_to_server,  config: { batch_size: 500 } }

  host_metrics:
    enabled: true
    sources:
      - { ref: cpu }
      - { ref: memory }
      - { ref: disk }
      - { ref: network }
    processors:
      - { ref: tagging }
    reporters:
      - { ref: http_to_server }

  # ── 未来的扩展示例（本期 bundle 留空，但 DSL 已支持） ──
  # local_anomaly_detection:
  #   enabled: false
  #   sources:    [{ ref: tap, config: { from: host_metrics, kinds: [metric] } }]
  #   processors: [{ ref: ai_ewma_zscore, config: { window_s: 300, threshold: 3.0 } }]
  #   reporters:  [{ ref: http_to_server }]

# ── 第三方 .so 插件（P1 仅支持 .so，WASM 推后） ──
plugins:
  - { path: /opt/zerotrace/plugins/my_custom_proto.so, kind: processor }

# ── 通信契约 ─────────────────────────────────
server:
  endpoint: https://zerotrace.example.com
  api_key:  ${ZEROTRACE_API_KEY}    # 从环境变量
  heartbeat_interval: 5s
  config_poll_interval: 30s

# ── 资源管控 ─────────────────────────────────
limits:
  cpu_quota: 1.0
  memory_mb: 512
```

### 6.2 PipelineExecutor

```rust
// crates/zerotrace-runtime/src/pipeline.rs
pub struct PipelineExecutor {
    name: String,
    sources:    Vec<Box<dyn Source>>,
    processors: Vec<Box<dyn Processor>>,
    reporters:  Vec<Box<dyn Reporter>>,
    channels:   Vec<mpsc::Sender<SignalBatch>>,  // 串接
}

impl PipelineExecutor {
    /// 启动期：从 YAML + World + Registry 构建并启动
    pub fn build(spec: &PipelineSpec, world: &World) -> Result<Self> {
        // 1. 用 ref 名从 Registry 解析每个组件
        // 2. 静态校验：sources[i].signals ⊆ processors[0].accepts，依次
        // 3. 创建 N-1 个 mpsc，串接
        // 4. 注册 lifecycle 钩子
    }

    /// 启动：每个组件一个 tokio task；source → proc → reporter 串行流转
    pub async fn run(self, ctx: LifecycleCtx) -> Result<()> {
        // sources 产数据 → 通道 → processors → 通道 → reporters
        // 任意环节 shutdown 时优雅排空（drain）
    }
}
```

**调度模型**：纯 tokio async，每个组件独占一个 task，组件间用 `tokio::sync::mpsc::channel(N)`（N 配置可调，默认 4096）串接，**天然背压**：reporter 处理不过来 → channel 塞满 → processor 阻塞 → source 阻塞 → 应用层感知不到数据丢失，监控里 channel 长度做指标。

---

## 7. Crate 组织（Vector 风格：胖主 bin + 瘦工具 crate）

### 7.1 设计原则

参考 Vector / cargo / wasmtime 的实践，crate 边界**只画在必须画的地方**：
1. **不同 target 架构 / no_std**（如 aya-ebpf kernel-side）；
2. **稳定可复用的工具层**（core / kernel / runtime 等）；
3. **重 native 依赖需要隔离**（暂无需求，packet 也可走主 bin mod）；

其余业务实现（具体的 source / processor / reporter / bundle）作为**主 bin 的 mod**，加新功能 = 加 .rs 文件，不开 crate。

### 7.2 目录布局

```
zerotrace-agent/
├── Cargo.toml                              # workspace 根
├── rust-toolchain.toml
├── deny.toml
├── xtask/                                  # 构建辅助
├── workspace-hack/                         # cargo-hakari
│
├── crates/                                 # 瘦工具 crate（稳定、可复用、独立可测）
│   # ── L1 Kernel（DI 核心） ─────────────────
│   ├── zerotrace-core/                     # Signal / SignalKind / SignalBatch / Error
│   ├── zerotrace-kernel/                   # World / SystemParam / Lifecycle / Bundle trait / ConfigBus
│   │
│   # ── L2 Runtime ─────────────────────────────
│   ├── zerotrace-runtime/                  # PipelineExecutor / BundleLoader
│   ├── zerotrace-config/                   # YAML + schema + hot-reload
│   ├── zerotrace-plugin-abi/               # .so 插件 C ABI（unix）
│   │
│   # ── L2 共享基础设施 ────────────────────────
│   ├── zerotrace-forwarder/                # agent→server HTTP 短连接 forwarder（reqwest async）:
│   │                                       #   控制面 register/heartbeat/sync/time + 数据面 uplink(wire帧→/data/ingest)
│   ├── zerotrace-platform/                 # K8s / libvirt / IMDS / proc fs 辅助
│   ├── zerotrace-debug/                    # debug socket + ctl 协议
│   │
│   # ── 强制独立 crate（target/no_std 不兼容主 bin） ──
│   ├── zerotrace-ebpf-kernel/              # aya-ebpf kernel-side, no_std, target = bpfel-unknown-none
│   │   ├── Cargo.toml
│   │   ├── build.rs                        # 编译 shim/*.c + bindgen
│   │   ├── shim/                           # DeepTrace 风格 C shim 桥接 CO-RE
│   │   │   ├── shim.c                      # SHIM(struct, member) 声明
│   │   │   ├── shim.h                      # SHIM 宏定义（BPF_CORE_READ 包装）
│   │   │   ├── types.h
│   │   │   └── include/                    # vmlinux.h 子集
│   │   └── src/                            # aya-ebpf Rust 内核代码
│   │       ├── lib.rs
│   │       ├── syscall/                    # read.rs / write.rs / sendto.rs / recvfrom.rs / sendmsg.rs / recvmsg.rs / close.rs
│   │       ├── tls/                        # openssl.rs / go.rs / syscall.rs / maps.rs
│   │       └── types.rs                    # SocketEvent / TlsPayload 等
│   │
│   # ── 既有 crate（保留不动） ───────────────────
│   ├── public/
│   ├── public-derive/
│   ├── public-derive-internals/
│   ├── trace-utils/
│   └── enterprise-utils/
│
├── src/                                    # 胖主 bin crate（业务功能加这里）
│   ├── main.rs                             # 仅 wiring：parse args → load bundles → spawn runtime
│   ├── lib.rs
│   │
│   ├── collectors/                         # ★ 所有 Source 实现
│   │   ├── mod.rs
│   │   ├── ebpf/                           # aya 用户态 loader（kernel-side 在独立 crate）
│   │   │   ├── mod.rs
│   │   │   ├── loader.rs                   # aya::Bpf::load
│   │   │   ├── socket_source.rs            # Source<Output = SocketEvent>
│   │   │   ├── tls/                        # TLS source + 三种 attacher
│   │   │   │   ├── mod.rs / source.rs
│   │   │   │   ├── openssl_attacher.rs     # 覆盖 BoringSSL（同 ABI）
│   │   │   │   └── go_attacher.rs
│   │   │   ├── btf_resolver.rs             # btfhub 子集加载
│   │   │   └── legacy/                     # 老 C 路径桥接，渐进迁移期保留
│   │   ├── proc/                           # /proc 主机指标
│   │   │   ├── mod.rs
│   │   │   ├── cpu.rs / memory.rs / disk.rs / network.rs
│   │   │   └── fixtures/                   # 测试 fixture
│   │   ├── packet/                         # AF_PACKET / libpcap
│   │   │   ├── mod.rs
│   │   │   ├── local.rs / multins.rs
│   │   │   ├── mirror.rs                   # 可选
│   │   │   └── analyzer.rs                 # 可选
│   │   └── integration/                    # OTel / Prometheus / Datadog / Telegraf / SkyWalking
│   │       ├── mod.rs / server.rs / receiver.rs
│   │       ├── otel.rs / prometheus.rs / datadog.rs / telegraf.rs / skywalking.rs
│   │
│   ├── processors/                         # ★ 所有 Processor 实现
│   │   ├── mod.rs
│   │   ├── flow.rs                         # 五元组流聚合
│   │   ├── l7/                             # 协议解析
│   │   │   ├── mod.rs
│   │   │   ├── http1.rs / http2.rs / grpc.rs / mysql.rs / postgres.rs
│   │   │   ├── redis.rs / dns.rs / kafka.rs / mongo.rs / amqp.rs / tls_handshake.rs
│   │   ├── reorder.rs
│   │   ├── reassembly.rs
│   │   ├── sql_obfuscate.rs
│   │   ├── trace_assemble.rs
│   │   └── tagging/                        # cloud_tag / k8s_tag / host_tag
│   │
│   ├── reporters/                          # ★ 所有 Reporter 实现
│   │   ├── mod.rs
│   │   ├── http.rs                         # 到 server
│   │   ├── file.rs                         # 调试
│   │   ├── stdout.rs                       # 调试
│   │   ├── factory.rs                      # UnifiedSenderFactory
│   │   └── npb_forward.rs                  # 可选
│   │
│   ├── bundles/                            # ★ Bundle 注册（按业务域）
│   │   ├── mod.rs                          # pub fn all_available() -> Vec<Box<dyn Bundle>>
│   │   ├── core.rs                         # 必选
│   │   ├── host_metric.rs
│   │   ├── microservice.rs
│   │   ├── packet_capture.rs               # 可选
│   │   ├── mirror.rs                       # 可选
│   │   ├── cloud_platform.rs
│   │   ├── integration.rs
│   │   ├── debug.rs
│   │   ├── ai.rs                           # 占位：register() 空实现
│   │   └── security.rs                     # 占位
│   │
│   ├── extensions/                         # 扩展点示例
│   │   ├── mod.rs
│   │   └── dummy_plugin_example.rs
│   │
│   └── trident.rs                          # 渐进废弃，向 runtime 迁移
│
├── plugins/                                # 既有 17 个 plugin crate（保持原状，逐步并入 collectors/processors mod）
│
├── bin/
│   └── zerotrace-agent-ctl/                # CLI 二进制（唯一独立 bin；主 bin 即仓库根 crate）
│
├── tests/                                  # 跨 crate 集成测试
└── examples/                               # 配置示例 + 插件 demo
```

**workspace 成员总数**：8 工具 crate + 1 ebpf-kernel + 5 既有 + 17 plugins + xtask + workspace-hack + zerotrace-agent-ctl = **33**

### 7.3 依赖方向（严格单向，禁止环）

```
                       core
                        ▲
                        │
                      kernel
                        ▲
        ┌──────┬────────┼────────┬────────┐
        │      │        │        │        │
    runtime  config  platform   fwd      debug
        ▲      ▲        ▲        ▲        ▲
        └──────┴────┬───┴────────┴────────┘
                    │
              ebpf-kernel       plugin-abi
                    ▲                ▲
                    └────────┬───────┘
                             │
                    主 bin (zerotrace-agent)
                    含 collectors / processors / reporters / bundles / extensions
```

**约束**：
- `core` 零依赖（只 std + serde + thiserror）。
- `kernel` 仅依赖 `core` + tokio + parking_lot + async-trait。
- `ebpf-kernel` no_std，target = bpfel-unknown-none，**不被主 bin 直接 `use`**——主 bin 通过加载产物 `.bpf.o` 与之交互。
- 主 bin 不被任何 crate 反向依赖（顶层消费者）。

### 7.4 Bundle 模型与编译单元解耦

Bundle 是**运行时注册概念**，不是编译单元：

```rust
// src/bundles/mod.rs
pub trait Bundle: Send + Sync + 'static {
    fn id(&self) -> &'static str;
    fn register(&self, world: &mut World, registry: &mut LifecycleRegistry) -> Result<()>;
}

pub fn all_available() -> Vec<Box<dyn Bundle>> {
    vec![
        Box::new(core::CoreBundle),
        Box::new(host_metric::HostMetricBundle),
        Box::new(microservice::MicroserviceBundle),
        Box::new(packet_capture::PacketCaptureBundle),
        Box::new(mirror::MirrorBundle),
        Box::new(cloud_platform::CloudPlatformBundle),
        Box::new(integration::IntegrationBundle),
        Box::new(debug::DebugBundle),
        Box::new(ai::AiBundle),               // 空 register，预留
        Box::new(security::SecurityBundle),   // 空 register，预留
    ]
}

// src/main.rs
let cfg = config::load("/etc/zerotrace-agent.yaml")?;
for b in bundles::all_available() {
    if cfg.enabled_bundles.contains(b.id()) {
        b.register(&mut world, &mut registry)?;
    }
}
```

YAML 用 `bundles: [core, host-metric, microservice]` 控制启用；行为与 v1 提案完全等价。

### 7.3 workspace `Cargo.toml` 模板

```toml
[workspace]
resolver = "2"
members = ["crates/*", "plugins/*", "bin/*", "xtask"]

[workspace.package]
edition      = "2021"
rust-version = "1.78"
license      = "Apache-2.0"

[workspace.dependencies]
tokio        = { version = "1.40", features = ["full"] }
async-trait  = "0.1"
parking_lot  = "0.12"
arc-swap     = "1.7"
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
serde_yaml   = "0.9"
schemars     = "0.8"
thiserror    = "1"
anyhow       = "1"
tracing      = "0.1"
reqwest      = { version = "0.12", default-features = false, features = ["rustls-tls", "json", "zstd"] }

# 内部 crate 路径声明（一次性）
zerotrace-core         = { path = "crates/zerotrace-core" }
zerotrace-kernel       = { path = "crates/zerotrace-kernel" }
zerotrace-runtime      = { path = "crates/zerotrace-runtime" }
zerotrace-config       = { path = "crates/zerotrace-config" }
zerotrace-forwarder    = { path = "crates/zerotrace-forwarder" }
zerotrace-platform     = { path = "crates/zerotrace-platform" }
zerotrace-debug        = { path = "crates/zerotrace-debug" }
zerotrace-plugin-abi   = { path = "crates/zerotrace-plugin-abi" }
# ebpf-kernel 不需要 workspace.dependencies 声明：主 bin 通过 .bpf.o 产物消费，不直接 use

[workspace.lints.rust]
unsafe_code        = "deny"          # 在 src/collectors/ebpf/、src/collectors/packet/ 模块级 allow

[workspace.lints.clippy]
all      = "warn"
pedantic = "warn"
```

---

## 8. Agent ↔ Server 通信契约

### 8.1 协议形态

| 项 | 方案 |
|---|---|
| 传输 | HTTPS + zstd 压缩 |
| 鉴权 | header `X-Api-Key: <key>` + `X-Agent-Id: <uuid>` |
| 连接模型 | **短连接**（DeepFlow 原有双向 gRPC stream 删除） |
| 编码 | JSON（控制面）+ Protobuf（数据面，复用 message/ 子模块定义） |
| 时间同步 | agent 主动 GET `/api/v1/time` |
| 调试通道 | server 不主动连 agent；agent 本机调试走 `zerotrace-agent-ctl` UDP |

### 8.2 端点清单

```
POST /api/v1/agent/register
  body  : { os, kernel, arch, nics, bpf_caps, agent_version }
  resp  : { agent_id, initial_config_version }

POST /api/v1/agent/heartbeat                # 5s 一次
  body  : { agent_id, health, stats: {...} }
  resp  : { config_version }                # agent 比对，必要时拉新配置

GET  /api/v1/agent/config?since=<version>
  resp  : 304 未变 / 200 + 新 config_yaml

GET  /api/v1/time
  resp  : { server_unix_nano }

POST /api/v1/data/metric                    # 批量上报
POST /api/v1/data/trace
POST /api/v1/data/log
POST /api/v1/data/profile
POST /api/v1/data/event
POST /api/v1/data/custom                    # 给未来 AI/安全用
  header : Content-Encoding: zstd
  body   : protobuf
  resp   : 202 / 400 / 401 / 429
```

### 8.3 Server 侧改造（`../zerotrace-server`）

| 改动 | 涉及文件 |
|---|---|
| 新增 HTTP handler 包 | `server/controller/http/handler/zerotrace/v1/*.go` |
| API_KEY middleware | `server/controller/http/middleware/apikey.go` |
| MySQL schema 加 `users` / `api_keys` | `server/controller/db/mysql/migration/...` |
| vtap_group 默认改单点占位 | `server/controller/grpc/vtap/...` |
| 删除 remote_exec 入口 | `server/controller/grpc/remote_exec.go` |
| genesis 多云 API 默认 disable | `server/controller/genesis/*.go` |
| ingester / querier 路径不变 | (复用) |

---

## 9. 扩展点：AI / 安全 / 自定义协议

本期不实现，但**接缝**留好：

### 9.1 加新 SignalKind
```rust
// 未来 crates/zerotrace-bundle-ai/src/anomaly.rs
#[derive(Debug)]
pub struct AnomalyPoint { pub metric: String, pub ts: i64, pub score: f64 }

impl ErasedSignal for AnomalyPoint {
    fn kind_name(&self) -> &'static str { "ai.anomaly" }
    fn as_any(&self) -> &dyn Any { self }
}
// 通过 Signal::Custom(Arc::new(AnomalyPoint{...})) 注入
```

### 9.2 加新 Processor（最常见的扩展形态）
```rust
// 未来 crates/zerotrace-bundle-ai/src/processor.rs
pub struct EwmaZscoreProcessor { ... }

#[async_trait]
impl Processor for EwmaZscoreProcessor {
    fn accepts(&self)  -> &[SignalKind] { &[SignalKind::Metric] }
    fn produces(&self) -> &[SignalKind] { &[SignalKind::Custom("ai.anomaly")] }
    async fn process(&mut self, batch: SignalBatch) -> Result<SignalBatch> { ... }
}
```

注册到 Bundle → YAML pipeline 引用即可工作，**不触碰任何已有 crate**。

### 9.3 加新 Source（如安全采集）
```rust
// 未来 crates/zerotrace-bundle-security/src/fim.rs
pub struct FimInotifySource { ... }

#[async_trait]
impl Source for FimInotifySource {
    type Output = SecurityEvent;
    fn signals(&self) -> &[SignalKind] { &[SignalKind::Event] }
    async fn run(&mut self, sink: SignalSink<SecurityEvent>) -> Result<()> { ... }
}
```

### 9.4 加新 Reporter（如发到 Kafka）
同模式，实现 `Reporter` trait + 注册到 Bundle。

### 9.5 .so 第三方插件
```rust
// 第三方编译为 cdylib，导出 C ABI 注册函数
#[no_mangle]
pub extern "C" fn _zerotrace_register(reg: *mut PluginRegistry) {
    let reg = unsafe { &mut *reg };
    reg.add_processor("my_proto_parser", Box::new(MyProtoParser::new()));
}
```

ABI 稳定性承诺：**`zerotrace-plugin` crate 一旦 1.0，C ABI 不再 break**。

---

## 10. 迁移路径：与现有代码共存

### 10.1 不破坏现状的策略

参照 `todo.md §11` 的并存模型：

```rust
// src/trident.rs（保留，但瘦身）
pub struct AgentComponents {
    // 旧字段保留——未迁移完的组件继续用
    pub config: ModuleConfig,
    pub dispatcher_components: Vec<DispatcherComponent>,
    // ... 50+ 字段中"未迁完"的部分

    // 新增：kernel 容器
    pub world:    Arc<World>,
    pub registry: Arc<Mutex<LifecycleRegistry>>,
    pub pipelines: Vec<PipelineExecutor>,
}
```

- 试点组件迁完后，从旧字段中删除，注册到 `world`；
- 启动流程末尾：`scheduler.run_startup(&world).await`；
- 停止流程开头：`scheduler.run_shutdown(&world).await`；
- 任意时点，旧 callback 模型与新 ConfigBus 可同时存在。

### 10.2 试点顺序（按风险递增）

| 阶段 | 试点组件 | 价值 | 风险 | 来源 |
|---|---|---|---|---|
| W1–W2 | 基础设施（kernel/runtime/core 三个 crate 落地） | 必要 | 低 | 新建文件 |
| W3 | **MetricServer** → 5 个独立 Receiver Component | 高（验证框架） | 低（5 协议有清晰边界） | todo.md §7 |
| W4 | HTTP 短连接 + API_KEY → 替换 gRPC stream | 高 | 中（涉及 rpc/sender 路径） | PPT slide 19 |
| W5–W8 | host_metric bundle + ebpf bundle + 包重组重排 | 高 | 低 | PPT slide 19, 27 |
| W9–W10 | eBPF 框架切 aya + Pixie TLS（调用栈关联法） | 极高 | 高 | PPT slide 19 + DeepTrace shim 模式 |
| W11 | **UniformSender 工厂化**（30 处队列样板 → factory.spawn） | 高 | 中 | todo.md §8 |
| W12 | **ConfigHandler 拆解**（435 个 diff → ConfigBus 订阅者） | 中 | 中 | todo.md §9 |
| W13–W14 | 部署、多 OS 兼容（CO-RE + btfhub） | 必要 | 低 | PPT slide 35 |
| W15 | 长跑稳定性 | 必要 | 低 | PPT slide 35 |
| W16 | demo + 文档 + 收尾 | 必要 | 低 | PPT slide 35 |

---

## 11. 4 个月里程碑计划

### M0（W1–W2）：基础设施

| 周 | 任务 | 涉及文件 |
|---|---|---|
| W1.D1–D2 | workspace 重组，建空 crate 骨架 | 根 `Cargo.toml`、12+ 新 crate 目录 |
| W1.D3 | `zerotrace-core`：Signal / SignalKind / SignalBatch / Error | `crates/zerotrace-core/src/*.rs` |
| W1.D4 | `zerotrace-kernel`：World / SystemParam / Res / Cfg / Sender / Recv | `crates/zerotrace-kernel/src/*.rs` |
| W1.D5 | `zerotrace-kernel`：Lifecycle / LifecycleRegistry / ConfigBus | 同上 |
| W2.D1 | `zerotrace-kernel`：Bundle trait + ComponentDescriptor | 同上 |
| W2.D2 | `zerotrace-runtime`：PipelineExecutor + BundleLoader | `crates/zerotrace-runtime/src/*.rs` |
| W2.D3 | `zerotrace-config`：YAML 解析 + schema 校验 + hot-reload | `crates/zerotrace-config/src/*.rs` |
| W2.D4 | 端到端 spike：`source-proc-cpu` + `reporter-stdout` 跑通 | `bin/zerotrace-agent/src/main.rs` |
| W2.D5 | M0 单测覆盖率 ≥ 80%；技术 RFC | `crates/zerotrace-kernel/tests/*.rs` |

**M0 交付**：内核框架 + 端到端 spike + 单测；老代码一行不动。

### M1（W3–W4）：MetricServer 拆解 + 通信改造

| 周 | 任务 | 涉及文件 |
|---|---|---|
| W3.D1–D2 | 抽 `Receiver` trait；OtelReceiver / PrometheusReceiver 试点 | `crates/zerotrace-source-integration/src/*.rs` |
| W3.D3 | 剩余 5 个 receiver | 同上 |
| W3.D4 | `IntegrationServer` 用 receivers 动态路由替换 `MetricServer::start` 闭包 | `crates/zerotrace-source-integration/src/server.rs` |
| W3.D5 | `AgentComponents` 中 MetricServer 装配改走 BundleLoader | `src/trident.rs:3142-3174` |
| W4.D1 | `zerotrace-forwarder`：HTTP 短连接 forwarder（控制面端点 + 数据面 upload_frames） | `crates/zerotrace-forwarder/src/*.rs` |
| W4.D2 | HTTP Reporter 走 `zerotrace-forwarder`（SignalBatch→wire帧→/data/ingest） | `src/reporters/http.rs` |
| W4.D3 | server fork 加 HTTP handler 包（在 `../zerotrace-server`） | `server/controller/http/handler/zerotrace/v1/*.go` |
| W4.D4 | API_KEY middleware + MySQL 表 | 同上 |
| W4.D5 | M1 端到端：agent 注册→心跳→拉配置→上报 mock 数据 | 集成测试 |

**M1 交付**：5 路接入器拆分完成，通信改 HTTP 短连接，API_KEY 鉴权链路打通。

### M2（W5–W8）：业务采集补齐

| 周 | 任务 | 涉及文件 |
|---|---|---|
| W5 | `zerotrace-source-proc`：CPU/Mem/Disk/Net 178 项指标 + 单测 fixtures | `crates/zerotrace-source-proc/src/*.rs` |
| W6 | `processor::reassembly`：TCP 序号块化 + 滑动窗口 + 30s 超时 | `crates/zerotrace-processor/src/reassembly.rs` |
| W7 | `processor::reorder`：TCP 乱序重排 + 老化策略 | `crates/zerotrace-processor/src/reorder.rs` |
| W8 | `zerotrace-bundle-cloud-platform`：IMDS / K8s informer / libvirt | `crates/zerotrace-bundle-cloud-platform/src/*.rs` |

**M2 交付**：单机部署 agent + demo 微服务，10 分钟内完整 metric + trace 在 server 可查。

### M3（W9–W12）：eBPF + Pixie TLS + 算法 + 重构收尾

| 周 | 任务 | 涉及文件 |
|---|---|---|
| W9 | eBPF 框架切 aya：内核态 Rust（aya-ebpf）+ DeepTrace 风格 C shim 桥接 CO-RE 字段访问；btfhub 集嵌入；先迁 socket_trace 子集（read/write/sendto/recvfrom）；perf_profiler / go_http2 / files_rw 维持原 C 路径，后续阶段再迁 | `crates/zerotrace-source-ebpf/src/{ebpf,user,shim}/`；保留 `src/ebpf/`（旧 C 路径）至 M4 |
| W10 | Pixie 调用栈关联 TLS（OpenSSL/BoringSSL/Go） | `crates/zerotrace-source-ebpf/src/tls/*` |
| W11 | UniformSender 工厂化（todo.md §8） | `crates/zerotrace-reporter`、`src/trident.rs` 30 处队列样板 |
| W12 | ConfigHandler 拆解 → ConfigBus 订阅者（todo.md §9）；server 算法（trace 组装 + Z-score） | `src/config/handler.rs`、`../zerotrace-server/server/controller/algorithm/*.go` |

**M3 交付**：aya 框架接入 + socket_trace 子集迁移；Pixie 加密解析在 aya 上跑通；agent 重构关键指标达成（trident.rs < 3000 行）；server 算法可输出异常区间。**老 C 路径仍在编译路径里**，供未迁移的 probe（perf_profiler 等）继续工作。

### M4（W13–W16）：部署、验证、收尾

| 周 | 任务 |
|---|---|
| W13 | 一键部署脚本 + Docker / DaemonSet manifest；server compose 复用 DeepFlow（去前端） |
| W14 | 多 OS 矩阵验证（CentOS 7.9 metric only / CentOS 8 / Ubuntu 20.04 / 22.04 / Debian 11） |
| W15 | 长跑稳定性（3 台 VM + opentelemetry-demo，跑满一周） |
| W16 | demo（cURL + jq + asciinema 视频）+ 论文/文档 + `v0.1.0-rc1` |

---

## 12. 验收指标

### 12.1 必达（M0 + M1 + M2 部分）

- [ ] L1 内核 4 个 trait + World 编译通过，单测覆盖 ≥ 80%
- [ ] L3 Bundle 机制能装载至少 4 个 bundle（core / host-metric / microservice / debug）
- [ ] HTTP 短连接 + API_KEY 鉴权端到端通
- [ ] MetricServer 拆分为 ≥ 5 个 Receiver Component
- [ ] 主机指标 178 项全部正确采集

### 12.2 关键（M3）

- [ ] eBPF 在 ≥ 3 个内核版本上跑通（CO-RE + btfhub）
- [ ] Pixie 风格 TLS 解析在 OpenSSL 3.0 / BoringSSL / Go crypto/tls 三类客户端都拿到明文
- [ ] `src/trident.rs` 行数 < 3000（当前 4123）
- [ ] `src/config/handler.rs` `on_config` 拆为 10+ 个子函数
- [ ] server 算法模块输出异常区间，前端 mock 查询可读

### 12.3 验证（M4）

- [ ] 5 个 OS 矩阵全过（CentOS 7.9 仅 metric，其余全功能）
- [ ] 一周长跑 agent 无崩溃、RSS 稳定 < 300MB
- [ ] 两个 demo 完整可重放（异常 trace 定位、异常主机定位）

### 12.4 扩展性（架构层面，可演示）

- [ ] 写一个 dummy AI Processor（不实现真实算法），加进 `zerotrace-bundle-ai`，YAML 引用即生效，**不改任何已有 crate 一行代码**
- [ ] 写一个 dummy 安全 Source，同样不改已有代码即可启用
- [ ] 一个 .so 插件 demo（在 `examples/plugin-dummy/`）可加载

---

## 13. 风险与回滚

| 风险 | 缓解 |
|---|---|
| Kernel 抽象设计不当 → 后续每个 Component 写起来都难受 | M0 末做"加一个 dummy Component"练习；不顺手立刻迭代抽象 |
| Pixie 调用栈关联法在某些 syscall 路径失效 | 保留 DeepFlow 旧 uprobe SSL_write/SSL_read 走法作为降级路径 |
| btfhub-archive 体积大（数百 MB） | 仅嵌入用户实际部署的 OS 子集；其余按需在线下载到 `/var/lib/zerotrace/btf/` |
| 4 个月不够 | M3 的"ConfigHandler 拆解"可缩水到只拆 `inputs.cbpf` 子树；M4 的"5 OS 矩阵"可只覆盖 3 个 |
| Pipeline async 调度有热点 | 提供 `Source::fast_path` 逃生口，绕开 trait object，直接给 sink 喂 `&[Packet]` |
| 上游 DeepFlow merge 冲突 | 改动尽量集中在新建 crate 中，老 `src/` 文件仅做最小切口 |

每个 M 结束打 git tag（`m0-done` / `m1-done` ...），允许从任意里程碑回滚。

---

## 14. 附录：与 todo.md 的对应关系

| todo.md 章节 | 本设计映射 |
|---|---|
| §1.1 World | §3.1 World（基本沿用，改 RwLock） |
| §1.2 System | 本设计**不引入 System**——agent 的运行模型是"长生命周期 task"而非"周期 tick system"，不适合 Bevy 的 Scheduler 抽象。改用 Lifecycle + PipelineExecutor |
| §1.3 SystemParam | §3.2 SystemParam（沿用，加 Cfg / Sender / Recv 特化） |
| §1.4 Res/ResMut | §3.2 Res / Cfg（沿用） |
| §1.5 Scheduler | 改为 §3.3 LifecycleRegistry（只管启停顺序，不管 tick） |
| §4 World 寄生策略 | §10 完全沿用 |
| §5 MetricServer 试点 | §11 M1 完全沿用 |
| §8 UniformSender 工厂化 | §11 M3 W11 |
| §9 ConfigHandler 解耦 | §11 M3 W12 |
| §11 兼容性策略 | §13 沿用 |

**本设计相对 todo.md 的扩展**：
1. 加入 Datadog `comp/` 的**接口/实现分离**与 **noop/mock** 规约（§4.1）；
2. 加入 **Bundle 层**作为业务能力的分组边界（§5）；
3. 加入 **Pipeline DSL** 作为用户面（§6）；
4. 加入 **PPT 业务路线**（HTTP 短连接、Pixie TLS、CO-RE、server 算法等）；
5. 加入 **多 crate workspace** 组织规范（§7）。

**本设计相对 todo.md 的简化**：
1. 删除 Bevy 的 `System` / `IntoSystem` / `FunctionSystem`——agent 不需要"裸函数自动变 system"的能力；
2. 删除 Scheduler 的 startup / update 二分——agent 没有 tick 概念。

---

## 15. eBPF 框架路线（v1.1 修订，2026-05-28）

### 15.1 决策

放弃 v1 原计划的"`libbpf-cargo` 编译 .bpf.c + 手工 CO-RE 化"，改用 **aya（内核态 Rust）+ DeepTrace 风格 C shim 桥接 CO-RE struct 字段访问**。

### 15.2 理由

- 现状 `src/ebpf/` 1.5 万行 C + 5 个内核版本宏分支（`LINUX_VER_3_10_0` / `5_2_PLUS` / `5_15_PLUS` / `KYLIN` / `KFUNC`）+ `libbcc` 运行时 → 维护成本高、build 不可复现、与 SaaS agent 形态相悖。
- aya 提供纯 Rust BPF loader、自带 CO-RE relocation + BTF 解析，去掉 libbcc / libdwarf / GoReSym 等 native 依赖。aya-ebpf 覆盖所有现用 probe type（fentry/fexit / kprobe / tracepoint / perf_event / uprobe）与 map type（PerfEventArray / RingBuf / HashMap / StackTrace 等）。
- aya 唯一痛点（CO-RE struct 字段访问宏不如 C `BPF_CORE_READ` 优雅）被 **DeepTrace 的 shim 模式**完美解决：百行级 C 代码声明 `SHIM(struct, member)`，clang 编译带 BTF 后 bindgen 导出 Rust FFI 函数，aya-ebpf Rust 代码直接调用，CO-RE relocation 由 libbpf relocator 兜底。

### 15.3 范围与节奏

- **M3 W9–W10**：搭 aya + shim 骨架；迁移 `socket_trace.bpf.c` 子集（4–6 个核心 syscall hook）+ Pixie TLS 用 aya 实现。
- **M3 / M4 之外**：`perf_profiler` / `go_http2` / `go_tls` / `files_rw` / `uprobe_base` 继续走老 C 路径（与 aya 路径**并存**），后续阶段迁移。
- **删除 `src/ebpf/Makefile`、`bintobuffer`、9 个 `static mut`、5 个 `LINUX_VER_*` 宏分支** —— 推迟到所有 probe 都迁完 aya 后（≥ 0.2.0 版本）。

### 15.4 与 §11 M3 W9–W10 任务表的对应

见 `todo.md §5.1 W9` 重写后的 T4.1–T4.4。

---

*文档版本：draft v1.1*
*v1 → v1.1 变更：eBPF 路线由 libbpf-cargo 改为 aya + DeepTrace shim，本文件 §5.3 / §7.1 / §10.2 / §11 / §15 同步更新*
*作者：基于 picture.pptx + Bevy DI 旧方案（draft1.md）+ Datadog `comp/` 框架 + DeepTrace 仓库实地考察 + 用户反馈综合整理*
