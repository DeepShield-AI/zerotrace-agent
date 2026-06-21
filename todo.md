# ZeroTrace 任务清单

## 0. 约定

### 0.1 任务格式

```
### T<阶段>.<序号> <任务名>  [W<周>.D<日> – W<周>.D<日>] [<天>d] [<状态>]
- 目标 : 一句话说明
- 涉及 : 文件 1 / 文件 2 / ...
- 修改 : 具体改什么
- 验收 : 可机器检验的判据（命令 / 文件存在 / 行数 / 测试通过等）
- 依赖 : 上游任务 ID
```

### 0.2 状态标记

- `[ ]` 待做
- `[~]` 进行中
- `[x]` 完成
- `[!]` 受阻（注明原因）
- `[-]` 已废弃

### 0.3 阶段

每个里程碑结束必须通过下列检查才能进入下一个：
1. `cargo xtask check` 全绿（fmt + clippy + nextest + deny）

---

## 1. 全局准备工作（W0）

### T0.0 脚手架就绪检查  [W0]  [0.5d]  [x]
- 目标 : 确认本次新增的脚手架文件齐备
- 涉及 : `rust-toolchain.toml` / `rustfmt.toml` / `clippy.toml` / `deny.toml` / `typos.toml` / `.editorconfig` / `.config/hakari.toml` / `workspace-hack/` / `xtask/` / `.github/workflows/quality.yml`
- 验收 :
  - `cargo build -p xtask` 通过
  - `cargo xtask --help` 列出子命令
  - 现有 `cargo build --no-default-features` 不退化

### T0.1 安装工具链  [W0]  [0.5d]  [ ]
- 目标 : 本机/CI 镜像装齐所需 cargo 子命令
- 涉及 : 工作机环境
- 修改 :
  ```bash
  cargo install cargo-deny cargo-hakari cargo-nextest cargo-machete typos-cli
  rustup component add rustfmt clippy rust-src
  ```
- 验收 : `cargo deny --version && cargo hakari --version && cargo nextest --version` 全部就绪

---

## 2. M0：基础设施搭建（W1–W2，10 工作日）

> **里程碑目标**：搭出 L1 Kernel 抽象（World / SystemParam / Lifecycle / ConfigBus）+ L2 Runtime 骨架（PipelineExecutor / BundleLoader），跑通"1 个 Source → 1 个 Reporter"端到端最小 demo。**老代码一行不动。**

### 2.1 Week 1：内核抽象与 workspace 重组

#### T1.1 workspace 重组：新建瘦工具 crate 骨架 + 主 bin mod 树  [W1.D1 – W1.D2]  [2d]  [ ]
- 目标 : 8 个新工具 crate（+ ebpf-kernel 留到 M3 W9 建）+ 主 bin 的 mod 目录布局就位
- 涉及（**新建工具 crate**，每个含 `Cargo.toml` + `src/lib.rs` 占位）:
  - `crates/zerotrace-core/`
  - `crates/zerotrace-kernel/`
  - `crates/zerotrace-runtime/`
  - `crates/zerotrace-config/`
  - `crates/zerotrace-forwarder/`
  - `crates/zerotrace-platform/`
  - `crates/zerotrace-debug/`
  - `crates/zerotrace-plugin-abi/`
- 涉及（**新建主 bin mod 目录**，每个含 `mod.rs` 占位）:
  - `src/collectors/{mod.rs, ebpf/mod.rs, proc/mod.rs, packet/mod.rs, integration/mod.rs}`
  - `src/processors/{mod.rs, l7/mod.rs, tagging/mod.rs}`
  - `src/reporters/mod.rs`
  - `src/bundles/mod.rs`
  - `src/extensions/mod.rs`
- 涉及（**修改**）:
  - 根 `Cargo.toml` 的 `[workspace.dependencies]` 加入 8 个新 crate 的 path 声明
  - `src/lib.rs` 在 `mod` 列表中追加 `collectors / processors / reporters / bundles / extensions`
- 修改 :
  - 每个新 crate 的 lib.rs 仅含 `// stub` 占位
  - 每个新 crate 的 Cargo.toml 声明 `[package]` + `edition.workspace = true` + `rust-version.workspace = true`
  - mod 目录的 `mod.rs` 只含 `// stub`
- 验收 :
  - `cargo metadata --format-version 1 | jq '.workspace_members | length'` 比改造前多 8（原值 + 8）
  - `cargo build --no-default-features` 仍然通过
  - `src/collectors/ebpf/mod.rs` 等占位文件存在

#### T1.2 `zerotrace-core`：Signal 体系  [W1.D3]  [1d]  [ ]
- 目标 : 信号枚举、SignalKind、SignalBatch、Error
- 涉及（新建）:
  - `crates/zerotrace-core/src/signal.rs` — `enum Signal` + `trait ErasedSignal`
  - `crates/zerotrace-core/src/kind.rs` — `enum SignalKind` + `Custom(&'static str)`
  - `crates/zerotrace-core/src/batch.rs` — `struct SignalBatch { kind, items, deadline }`
  - `crates/zerotrace-core/src/error.rs` — `enum Error` + `pub type Result<T>`
- 修改 :
  - `Signal` 含 6 个变体：Metric / Trace / Log / Profile / Event / Custom(Arc<dyn ErasedSignal>)
  - 全部 `#[derive(Debug, Clone)]`；Custom 不能 derive，手写 Debug
- 验收 :
  - `cargo test -p zerotrace-core` 含 ≥ 5 个单测（含 Custom 注入与 downcast）
  - `cargo doc -p zerotrace-core --no-deps` 无 warning

#### T1.3 `zerotrace-kernel`：World + SystemParam  [W1.D4]  [1d]  [ ]
- 目标 : DI 容器与参数自动注入
- 涉及（新建）:
  - `crates/zerotrace-kernel/src/world.rs` — `struct World` + insert/get/contains
  - `crates/zerotrace-kernel/src/param.rs` — `trait SystemParam` + `Res<T>` / `Cfg<T>` / `Sender<T>` / `Recv<T>`
  - `crates/zerotrace-kernel/src/error.rs` — `enum KernelError`
- 修改 :
  - `World` 内部 `parking_lot::RwLock<HashMap<TypeId, Arc<dyn Any+Send+Sync>>>`
  - `Res<T>` 是 `Arc<T>` 的 newtype，`Deref<Target = T>`
  - `SystemParam` 用宏对 1..=12 元 tuple 一次性 impl
- 验收 :
  - 单测 ≥ 8 个：插入/读取/替换/缺失/并发读/Cfg.load/tuple/嵌套 Arc
  - `cargo bench -p zerotrace-kernel` 中 `world_get` < 100 ns

#### T1.4 `zerotrace-kernel`：Lifecycle + ConfigBus  [W1.D5]  [1d]  [ ]
- 目标 : 生命周期钩子注册器 + 订阅式配置变更
- 涉及（新建）:
  - `crates/zerotrace-kernel/src/lifecycle.rs` — `trait Lifecycle` + `LifecycleRegistry` + `Health`
  - `crates/zerotrace-kernel/src/config_bus.rs` — `enum ConfigChange` + `trait ConfigSubscriber` + `enum Action`
- 修改 :
  - `Lifecycle` 全部方法 `async`，默认实现返回 `Ok(())`
  - `LifecycleRegistry::stop_all` 必须按注册逆序调用
  - `ConfigBus::dispatch` 串行调用 subscriber，遇 `Action::RestartAgent` 立即返回
- 验收 :
  - 单测验证启动顺序、停止逆序、订阅过滤
  - 模拟"3 个组件其中 1 个启动失败"场景，确认前 2 个被回滚 stop

### 2.2 Week 2：Bundle、Runtime、端到端 spike

#### T1.5 `zerotrace-kernel`：Bundle trait + ComponentDescriptor  [W2.D1]  [1d]  [ ]
- 目标 : Bundle 装载抽象
- 涉及（新建）:
  - `crates/zerotrace-kernel/src/bundle.rs` — `trait Bundle` + `ComponentDescriptor` + `BundleSet`
- 修改 :
  - `Bundle::components(&self) -> Vec<ComponentDescriptor>`
  - `ComponentDescriptor` 含 `factory: Box<dyn Fn(&World, &mut LifecycleRegistry) -> Result<...>>`
- 验收 :
  - 用 dummy Bundle 注册 2 个组件，BundleSet::load 后 World 内可取到资源

#### T1.6 `zerotrace-runtime`：PipelineExecutor  [W2.D2]  [1d]  [ ]
- 目标 : Source → Processor → Reporter 调度器
- 涉及（新建）:
  - `crates/zerotrace-runtime/src/pipeline.rs` — `struct PipelineExecutor` + `struct PipelineSpec`
  - `crates/zerotrace-runtime/src/loader.rs` — `struct BundleLoader`
- 修改 :
  - PipelineExecutor::build 做信号类型静态校验（不匹配返回 Err）
  - 组件间通过 `tokio::sync::mpsc::channel(4096)` 串接
  - shutdown 时优雅 drain（reporter 处理完 channel 内剩余 batch）
- 验收 :
  - 单测：3 节点 pipeline（dummy source → echo processor → counting reporter）能处理 1000 条信号
  - 注入"processor accepts 与 source signals 不匹配"配置，build 阶段返回 Err

#### T1.7 `zerotrace-config`：YAML 解析 + hot-reload  [W2.D3]  [1d]  [ ]
- 目标 : 加载配置文件，发出 ConfigChange 事件
- 涉及（新建）:
  - `crates/zerotrace-config/src/lib.rs` — `struct AgentConfig` + `pub fn load(path: &Path) -> Result<AgentConfig>`
  - `crates/zerotrace-config/src/watcher.rs` — `struct ConfigWatcher`（SIGHUP + notify crate 监听文件变更）
- 修改 :
  - `AgentConfig` 用 `#[derive(Serialize, Deserialize, JsonSchema)]`
  - `ConfigWatcher::start` 返回 `mpsc::Receiver<ConfigChange>`
- 验收 :
  - 单测：读 `examples/agent.yaml` 成功；改文件后 watcher 在 100ms 内推 ConfigChange

#### T1.8 端到端 spike：proc CPU → stdout  [W2.D4]  [1d]  [ ]
- 目标 : 用最简单的 Source + Reporter 验证整链路
- 涉及（新建，主 bin mod 内）:
  - `src/collectors/proc/cpu.rs` — `RealCpuCollector` + `NoopCpuCollector` + `#[cfg(test)] MockCpuCollector`
  - `src/reporters/stdout.rs`
  - `src/bundles/core.rs` — `CoreBundle::register` 注册上述组件
  - `src/main.rs` — 仅 wiring：load config → load enabled bundles → spawn runtime
- 修改 :
  - CPU collector 解析 `/proc/stat`，每秒一次输出 user/system/iowait
  - stdout reporter 将 SignalBatch 用 `serde_json` 打印
  - main.rs 默认 enable `core` bundle，跑一个 `host_metrics_spike` pipeline
- 验收 :
  - `cargo run` 后 stdout 每秒看到一行 JSON metric
  - `Ctrl-C` 优雅停机（lifecycle.stop_all 全部 Ok）

#### T1.9 M0 收尾  [W2.D5]  [1d]  [ ]
- 目标 : 测试 + tag + 复盘
- 修改 :
  - `cargo xtask check` 全绿
  - `cargo nextest run --workspace --no-default-features` ≥ 30 个测试
  - `git tag m0-done`
- 验收 :
  - Tag 推到 dev 分支
  - 本节所有任务标记 [x]

---

## 3. M1：通信改造 + MetricServer 拆解（W3–W4，10 工作日）

> **里程碑目标**：（1）把 `MetricServer` 拆为 5 个独立 Receiver Component，验证 L2 Component 规约；（2）agent ↔ server 通信从双向 gRPC stream 改为 HTTP 短连接 + API_KEY。

### 3.1 Week 3：MetricServer 拆解

#### T2.1 Receiver trait + integration mod 骨架  [W3.D1]  [1d]  [ ]
- 目标 : 定义 Receiver trait，搭主 bin 内 integration mod 框架
- 涉及（新建）:
  - `src/collectors/integration/receiver.rs` — `trait Receiver: Source`
  - `src/collectors/integration/server.rs` — `IntegrationServer` 骨架（仅占位）
- 修改 :
  - `Receiver::route_prefix() -> &'static str`（HTTP 路径前缀）
  - `Receiver::handle(req) -> Result<Output>`

#### T2.2 实现 5 个 Receiver  [W3.D2 – W3.D4]  [3d]  [ ]
- 目标 : OTel / Prometheus / Datadog / Telegraf / Profile 五种接入
- 涉及（新建，每个 receiver 一个目录、3 个实现文件）:
  - `src/collectors/integration/otel/{mod.rs, real.rs, noop.rs, mock.rs}`
  - `src/collectors/integration/prometheus/{mod.rs, real.rs, noop.rs, mock.rs}`
  - `src/collectors/integration/datadog/{mod.rs, real.rs, noop.rs, mock.rs}`
  - `src/collectors/integration/telegraf/{mod.rs, real.rs, noop.rs, mock.rs}`
  - `src/collectors/integration/profile/{mod.rs, real.rs, noop.rs, mock.rs}`
- 涉及（迁移）:
  - 源码搬自 `src/integration_collector.rs:95-1338`（各协议解析逻辑）
- 修改 :
  - 每个 receiver 实现 `Source<Output = SpecificType>` + `Receiver` + `Lifecycle`
  - `noop.rs` 是空实现，配置 disable 时使用
  - `mock.rs` 在 `#[cfg(any(test, feature = "test-utils"))]` 下导出
- 验收 :
  - 每个 receiver 至少 3 个单测（解析 / 错误处理 / lifecycle）
  - 现有 OTel / Prometheus 集成测试用例迁移至新位置仍通过

#### T2.3 IntegrationServer：动态路由替代 hyper 闭包  [W3.D5]  [1d]  [ ]
- 目标 : 删除原 `MetricServer::start` 内的 service_fn 大闭包
- 涉及（实现）: `src/collectors/integration/server.rs`（在 T2.1 占位上落地）
- 涉及（删除/废弃）: `src/integration_collector.rs:1147-1318`（功能搬到 collectors/integration/ 后清理）
- 修改 :
  - `IntegrationServer::new(port)` + `add_receiver(Box<dyn Receiver>)`
  - `Lifecycle::on_start` 内根据 receivers 构建 router（axum 或继续 hyper）
  - 顶层 `src/integration_collector.rs` 改为薄壳，`pub use crate::collectors::integration::*;` 兼容期 re-export，M4 末删除
- 涉及（新建/修改 bundle）: `src/bundles/integration.rs` 注册上述 5 个 receiver
- 验收 :
  - 端到端：用 `curl` 发 OTel HTTP/protobuf，能收到并存到内部队列
  - benchmark：每秒 1000 条 OTel span 上报，CPU 较改造前 ±5% 内

### 3.2 Week 4：通信改造

#### T2.4 `zerotrace-forwarder`：HTTP 短连接 forwarder  [W4.D1]  [1d]  [ ]
> 命名：原 `zerotrace-rpc` 改名。HTTP 短连接非 RPC 语义，且涵盖数据面 uplink；取 Datadog "forwarder" 之意。
> 数据路线：定 **1.A** —— 数据面走 **wire 帧 → server `/data/ingest` → ingester 管线 → `flow_log.*`**（与原 TCP 落库格式一致），**不是** `submit_metric/submit_trace` 简化 DTO。
- 目标 : reqwest(async) 实现 控制面端点 + 数据面 upload_frames
- 涉及（新建）:
  - `crates/zerotrace-forwarder/src/lib.rs` — `struct Forwarder` + `ForwarderBuilder`
  - `crates/zerotrace-forwarder/src/config.rs` — `ForwarderConfig { base_url, api_key, timeout, retries, compression }`
  - `crates/zerotrace-forwarder/src/auth.rs` — `X-Api-Key` / `X-Agent-Id` 注入
  - `crates/zerotrace-forwarder/src/control.rs` — 控制面: `sync/heartbeat/register/query_time/k8s_cluster_id/gpid_sync/upgrade/plugin/remote_exec_{poll,result}`（protobuf, 复用 `message/agent` 类型；对应 server 已实现的 `/api/v1/agent/*`）
  - `crates/zerotrace-forwarder/src/data.rs` — 数据面: `upload_frames(&[u8])` → `POST /api/v1/data/ingest`（wire 帧 firehose, zstd 可选）
  - `crates/zerotrace-forwarder/src/error.rs` — `ForwarderError`
- 修改 :
  - 全 async；请求体可 zstd 压缩；timeout 默认 10s + 指数退避重试 3 次
  - `ZT_API_KEY` 环境变量优先于配置（Datadog 风格）
- 验收 :
  - 单测用 `wiremock` 模拟 server 响应；覆盖 200 / 204 / 401 / 429 / 500
  - `upload_frames` 发一帧 l7，server `/data/ingest` 收到并落 `flow_log.l7_flow_log`

#### T2.5 HTTP Reporter 走 forwarder  [W4.D2]  [1d]  [ ]
- 目标 : `src/reporters/http.rs` 用 `zerotrace-forwarder::Forwarder`
- 涉及（新建/修改）:
  - `src/reporters/http.rs`
- 涉及（删除）:
  - `src/sender/uniform_sender.rs` 的上传逻辑（暂保留 + `#[deprecated]`，M3 W11 删除）。
    注：当前已有的 `HttpTransport`/env 注入为过渡 stopgap，随 uniform_sender 一并退场。
- 修改 :
  - SignalBatch → 编码成 wire 帧（BaseHeader[+FlowHeader]+protobuf）→ `forwarder.upload_frames()`
  - 批量：达到 N 条或 T 秒触发上报
- 验收 :
  - 端到端：spike 配置改 HTTP reporter，server `flow_log.*` 在 10 秒内见到新行

#### T2.6 Server fork：HTTP handler 包  [W4.D3]  [1d]  [x] 已落地（实现与原计划有调整）
- 目标 : `../zerotrace-server` 加 agent HTTP 端点
- 实际落地（位置与原计划不同，见 zerotrace-server 仓库）:
  - 控制面 `controller/http/router/agent/zerotrace_sync.go` —— `/api/v1/agent/{sync,ntp,gpid_sync,genesis_sync,kubernetes_api_sync,kubernetes_cluster_id}`，**直接复用 gRPC trisolaris 处理逻辑**（protobuf）
  - 流式下载 `zerotrace_stream.go` —— `/api/v1/agent/{upgrade,plugin}`（长度前缀帧）
  - RemoteExecute 轮询 `/api/v1/agent/remote_exec/{poll,result}`
  - **数据面（定 1.A）** `zerotrace_ingest.go` —— `POST /api/v1/data/ingest`：收 agent wire 帧 → `libs/receiver.PutHTTPData` → ingester 管线 → `flow_log.*`（与 TCP 落库**字节级一致**）
  - 旧的简化 `/data/{metric,trace,log,...}`（zerotrace_data.go）保留但**非主路径**，1.A 下由 `/data/ingest` 取代
- 验收 : `/agent/sync` 返回有效 SyncResponse（已实测 Status=SUCCESS）；`/data/ingest` 落 `flow_log.l7_flow_log`（待 agent forwarder 接通后端到端）

#### T2.7 API_KEY 鉴权 middleware + 用户表  [W4.D4]  [1d]  [x] 已落地
- 目标 : server 启动鉴权
- 实际落地（zerotrace-server 仓库）:
  - `controller/http/router/agent/zerotrace_apikey.go` —— `APIKeyAuth()` gin middleware，sha256 比对 `api_keys.key_hash` 且 `revoked_at IS NULL` 才放行，挂在 `/api/v1` 组
  - `users`/`api_keys` 表经迁移框架落地：`migrator/schema/rawsql/mysql/issu/7.1.0.34.sql` + `DB_VERSION_EXPECTED=7.1.0.34`
- 验收（已实测）：无 key→401；错误 key→401；有效 key→200

#### T2.8 M1 收尾  [W4.D5]  [1d]  [ ]
- 目标 : 端到端联调 + 复盘 + tag
- 修改 :
  - 启 server + agent，跑 30 分钟，每秒上报 metric 无 401/500
  - 改 agent yaml 配置 `enabled: false` 某 receiver，SIGHUP 后该 receiver 被 stop
  - `git tag m1-done`
- 验收 :
  - §10.2 M1 验收指标全 ✓
  - 本文末尾追加 M1 复盘 200 字

---

## 4. M2：业务采集补齐（W5–W8，20 工作日）

> **里程碑目标**：主机指标补齐 178 项；包重组 + 乱序重排实现（DeepFlow 原代码空壳）；本机元数据 Source 完成（IMDS / K8s informer / libvirt）。

### 4.1 Week 5：主机指标 178 项

#### T3.1 CPU 指标（17 项）  [W5.D1]  [1d]  [ ]
- 涉及（修改 / 完善）:
  - `src/collectors/proc/cpu.rs`（已在 M0 T1.8 有骨架）
- 修改 :
  - 解析 `/proc/stat` 拿到 user/nice/system/idle/iowait/irq/softirq/steal/guest/guest_nice 共 10 项
  - 派生 7 项：cpu_usage_pct / user_pct / system_pct / iowait_pct / load_1 / load_5 / load_15（后三个读 `/proc/loadavg`）
- 验收 :
  - 单测：用 `src/collectors/proc/fixtures/proc_stat_*.txt` 4 个快照，断言所有 17 项数值正确

#### T3.2 内存指标（124 项）  [W5.D2]  [1d]  [ ]
- 涉及 : `src/collectors/proc/memory.rs`
- 修改 :
  - 解析 `/proc/meminfo` 全部字段（≈ 60 项）
  - 解析 `/proc/vmstat` 关键字段（≈ 50 项：page_in/out / swap / pgfault 等）
  - 派生 14 项：mem_usage_pct / swap_usage_pct / buffer_pct / cache_pct / ...
- 验收 : 单测 fixture ≥ 3 份，覆盖 NUMA / non-NUMA / cgroup v2

#### T3.3 磁盘指标（20 项）  [W5.D3]  [1d]  [ ]
- 涉及 : `src/collectors/proc/disk.rs`
- 修改 : 解析 `/proc/diskstats` + `/proc/mounts` + `statvfs`
- 验收 : 单测 fixture ≥ 2 份；含 nvme/sda/dm-* 设备

#### T3.4 网络指标（17 项）  [W5.D4]  [1d]  [ ]
- 涉及 : `src/collectors/proc/network.rs`
- 修改 : 解析 `/proc/net/dev` + `/sys/class/net/*/statistics/*`
- 验收 : 单测 + 真机抓 5 分钟数据对比 `ifconfig`

#### T3.5 host-metric Bundle 装配  [W5.D5]  [1d]  [ ]
- 涉及（新建）: `src/bundles/host_metric.rs`
- 修改 : 注册 4 个 collector + tagging processor + http reporter；提供 `default_pipelines() -> [host_metrics 模板]`
- 验收 : agent 启用 host-metric bundle，5 分钟数据全 178 项可在 server 查到

### 4.2 Week 6：TCP 包重组（packet_segmentation_reassembly）

#### T3.6 设计文档  [W6.D1]  [1d]  [ ]
- 涉及（新建）: `docs/design/reassembly.md`
- 修改 :
  - 描述 PerFlowReassembler 数据结构、超时策略（默认 30s）、内存上限（默认 64MB 全局）
  - 与 reorder 的串接顺序：reorder → reassembly → l7_parse
- 验收 : 至少 1 人 review 通过

#### T3.7 PerFlowReassembler 实现  [W6.D2 – W6.D3]  [2d]  [ ]
- 涉及（修改 / 完善）:
  - `plugins/packet_segmentation_reassembly/src/lib.rs`（现状空壳）
  - 新增 `plugins/packet_segmentation_reassembly/src/per_flow.rs`
- 修改 :
  - `struct PerFlowReassembler { syn_seq: u32, buffer: BTreeMap<u32, Bytes>, last_active: Instant, total_bytes: usize }`
  - `fn ingest(&mut self, seq: u32, payload: Bytes) -> Vec<Bytes>`（返回此次能放行的连续段）
  - 全局 `LruCache<FlowKey, PerFlowReassembler>` 限内存
- 验收 :
  - 单测：5 个场景（顺序 / 交错 / 重复 / 缺洞 / 超时）

#### T3.8 包成 Processor 接入 Pipeline  [W6.D4]  [1d]  [ ]
- 涉及（新建/修改）:
  - `src/processors/reassembly.rs` — wrap 上面的 PerFlowReassembler 实现 `Processor`
- 修改 :
  - `accepts: [SignalKind::Custom("raw_packet")]` / `produces: [SignalKind::Custom("reassembled_segment")]`
- 验收 :
  - 端到端：注入分片 HTTP/2 流量，能在下游 L7 parser 拿到完整 frame

#### T3.9 边界场景测试  [W6.D5]  [1d]  [ ]
- 涉及 : `tests/processor_reassembly_e2e.rs`
- 修改 : 用 pcap fixture（含 HTTP/2 over TLS、gRPC stream）跑端到端
- 验收 : `cargo nextest run -p zerotrace-processor reassembly_e2e` 全过

### 4.3 Week 7：TCP 乱序重排（reorder）

#### T3.10 设计 + 实现 ReorderBuffer  [W7.D1 – W7.D2]  [2d]  [ ]
- 涉及（修改 / 完善）: `plugins/reorder/src/lib.rs`（现状空壳）
- 修改 :
  - `struct ReorderBuffer { expected_seq: u32, holes: BTreeMap<u32, Bytes>, idle_since: Instant }`
  - 老化策略：单流 200ms 无活动则强制 flush
- 验收 : 单测覆盖乱序、丢包、重复、回绕（seq 32-bit wrap）

#### T3.11 包成 Processor  [W7.D3]  [1d]  [ ]
- 涉及（新建/修改）: `src/processors/reorder.rs`
- 验收 : 与 reassembly 串接成 reorder → reassembly → l7_parse 链路

#### T3.12 性能基线  [W7.D4]  [1d]  [ ]
- 涉及（新建）: `benches/processor_reorder.rs`
- 修改 : criterion harness；目标 100K pps 单核 < 50% CPU
- 验收 : benchmark 报告写入 `docs/perf/reorder_baseline.md`

#### T3.13 microservice Bundle 雏形  [W7.D5]  [1d]  [ ]
- 涉及（新建）: `src/bundles/microservice.rs`
- 修改 : 注册 reorder + reassembly + l7_parse + trace_assemble + http_reporter（其中 l7_parse / trace_assemble 暂用 noop，M3 真填实现）
- 验收 : agent 启用 microservice bundle，端到端 pipeline 能跑通空运转

### 4.4 Week 8：本机元数据 Source

#### T3.14 IMDS Tagger  [W8.D1]  [1d]  [ ]
- 涉及（新建）: `crates/zerotrace-platform/src/imds/{mod.rs,aws.rs,aliyun.rs,gcp.rs}`
- 修改 : 启动期一次性 HTTP GET metadata service；缓存到 World
- 验收 : 三家云的 mock metadata server 都能解析

#### T3.15 K8s informer（仅本节点）  [W8.D2 – W8.D3]  [2d]  [ ]
- 涉及（迁移 / 修改）:
  - `src/platform/kubernetes/` → `crates/zerotrace-platform/src/k8s/`
- 修改 :
  - 用 `kube::runtime::watcher`，`field_selector="spec.nodeName=$NODE_NAME"` 仅拉本节点 Pod
  - 暴露 `K8sCache` 资源到 World
- 验收 : minikube 上启动 agent，能正确关联本节点 Pod IP → Pod 名

#### T3.16 libvirt + 进程 Tagger 迁移  [W8.D4]  [1d]  [ ]
- 涉及（迁移）:
  - `src/platform/libvirt_xml_extractor.rs` → `crates/zerotrace-platform/src/libvirt.rs`
  - `src/platform/platform_synchronizer/` → `crates/zerotrace-platform/src/process.rs`
- 修改 : 仅迁移，签名改为 trait + impl 风格；保持行为不变
- 验收 : 现有相关单测全过

#### T3.17 cloud-platform Bundle  [W8.D5]  [1d]  [ ]
- 涉及（新建）: `src/bundles/cloud_platform.rs`
- 修改 : 装配 IMDS / K8s / libvirt / process 4 个 tagger
- 验收 : §10.3 M2 验收指标全 ✓；`git tag m2-done`

---

## 5. M3：eBPF 切 aya + Pixie TLS + 算法 + 重构收尾（W9–W12，20 工作日）

> **里程碑目标**：（1）eBPF 框架由 libbcc/C 切到 **aya（内核态 Rust）+ DeepTrace 风格 C shim** 桥接 CO-RE，先迁 socket_trace 子集；（2）Pixie 调用栈关联法 TLS 明文采集（基于 aya）；（3）server 算法模块（trace 组装 + 异常检测）；（4）消除 UniformSender 重复样板 + 局部拆解 ConfigHandler。
>
> **不在本里程碑范围**：`perf_profiler` / `go_http2` / `files_rw` / `uprobe_base` 维持现有 C 路径（不动），等 socket_trace + TLS 验证 aya 路线后，后续阶段再分批迁。

### 5.1 Week 9：aya 接入 + shim 桥接 + socket_trace 子集迁移

#### T4.1 aya 框架接入与依赖梳理  [W9.D1]  [1d]  [ ]
- 目标 : 建立 kernel-side 独立 crate + 主 bin 内 user-side mod，跑通 aya 工具链
- 涉及（新建独立 crate）:
  - `crates/zerotrace-ebpf-kernel/Cargo.toml`（独立 [package]，依赖 aya-ebpf + aya-log-ebpf，target = bpfel-unknown-none）
  - `crates/zerotrace-ebpf-kernel/build.rs`（aya-build 触发 + 后续 T4.2 接 clang/bindgen）
  - `crates/zerotrace-ebpf-kernel/src/lib.rs`（kernel-side Rust，空 lib.rs 占位）
- 涉及（修改主 bin）:
  - 根 `Cargo.toml` 的 `[workspace.dependencies]` 加 `aya = "0.13"` / `aya-log = "0.2"` / `aya-build = "0.1"`
  - 根 `[workspace.members]` 加 `crates/zerotrace-ebpf-kernel`
  - `src/collectors/ebpf/mod.rs`（M0 已建占位）→ 加 `pub mod loader; pub mod btf_resolver;` 等空 mod 声明
  - 主 bin 的 `[dependencies]` 加 `aya` + `aya-log`（user-side loader 用）
- 修改 :
  - kernel-side crate 仅一个空 lib.rs
  - user-side mod 仅 `pub fn placeholder() {}`
- 验收 :
  - `cargo build -p zerotrace-ebpf-kernel` 通过；产出物含 .bpf.o
  - `cargo build`（主 bin）能 link 通过
- 依赖 : T1.1（workspace 重组）

#### T4.2 DeepTrace 风格 C shim 桥接层  [W9.D2]  [1d]  [ ]
- 目标 : 给 aya-ebpf 提供 CO-RE struct 字段访问入口
- 涉及（新建）:
  - `crates/zerotrace-ebpf-kernel/build.rs`（参考 DeepTrace `agent/crates/ebpf-common/build.rs`：bindgen + clang -target bpf -g）
  - `crates/zerotrace-ebpf-kernel/shim/shim.h`（定义 `SHIM(struct, member)` / `SHIM_REF(struct, member)` 宏，展开为 `BPF_CORE_READ` 包装）
  - `crates/zerotrace-ebpf-kernel/shim/shim.c`（声明初始字段集合：`SHIM(sock, sk_family)` / `SHIM(sock, __sk_common.skc_daddr)` / `SHIM(task_struct, pid)` / `SHIM(task_struct, tgid)` / `SHIM(files_struct, fdt)` 等 10–15 条起步）
  - `crates/zerotrace-ebpf-kernel/shim/types.h`（vmlinux.h 子集，从 btfhub 任一 BTF 用 `bpftool gen vmlinux` 抽取）
- 修改 : build.rs 输出 `OUT_DIR/shim_bindings.rs` 供 aya-ebpf 子 crate include
- 验收 :
  - clang 编译产物 .bpf.o 含 BTF（`bpftool btf dump file shim.bpf.o` 可读）
  - bindgen 生成的 Rust 函数名形如 `shim_sock_sk_family(*const sock) -> u16`
- 依赖 : T4.1

#### T4.3 socket_trace 子集迁移：spike 阶段  [W9.D3]  [1d]  [ ]
- 目标 : 端到端跑通"aya tracepoint → ringbuf → 用户态 tokio 流"
- 涉及（新建）:
  - `crates/zerotrace-ebpf-kernel/src/syscall/write.rs` — `#[tracepoint]` hook `sys_enter_write`，读 fd + buf_ptr + count，发到 RingBuf
  - `crates/zerotrace-ebpf-kernel/src/lib.rs` — 声明 ringbuf map
  - `src/collectors/ebpf/loader.rs` — `Bpf::load_file` + `RingBuf::async_poll` 走 tokio
- 修改 :
  - kernel-side 用 `aya_ebpf::macros::tracepoint` + `aya_ebpf::programs::TracePointContext`
  - 用户态 spawn 一个 tokio task 消费 ringbuf，打印事件数
- 验收 :
  - 跑 agent，`echo hello > /tmp/x` 后用户态能打出 N 条 write 事件
  - 不引入任何 `static mut`
- 依赖 : T4.2

#### T4.4 socket_trace 子集迁移：核心 syscall  [W9.D4 – W9.D5]  [2d]  [ ]
- 目标 : 覆盖 socket data 主路径：`read / write / sendto / recvfrom / sendmsg / recvmsg`
- 涉及（新建）:
  - `crates/zerotrace-ebpf-kernel/src/syscall/{read,sendto,recvfrom,sendmsg,recvmsg}.rs`（参考 DeepTrace `agent/crates/observ-trace-ebpf/src/` 同名文件布局，每个 syscall 一个 .rs）
  - `crates/zerotrace-ebpf-kernel/src/types.rs` — `SocketEvent { tid, fd, syscall, ts_ns, len, dir }` 等
  - `src/collectors/ebpf/socket_source.rs` — 实现 `Source<Output = SocketEvent>` + Lifecycle
- 修改 :
  - kernel-side 用 `#[tracepoint]` 而非 fentry（覆盖 ≥ 4.18 内核）；fentry/fexit 路径作为 feature `fast-path` 留口，本期不实现
  - 用 shim 函数读 `task_struct->tgid` / `task_struct->pid`
  - 五元组：调用 shim 读 `sock` 字段；socket fd → sock 映射用 RawTracepoint `sock_init_data` 维护一张 BPF HashMap
- 验收 :
  - 起 `curl http://example.com`，agent 能上报 1 条完整 socket_event（含五元组、时间戳、payload 前 N 字节）
  - 在 Ubuntu 22.04（5.15）+ CentOS 8（4.18）双内核验证通过
- 依赖 : T4.3

#### T4.5 btfhub-archive 子集嵌入  [W9.D5（与 T4.4 并行）]  [0.5d，分摊]  [ ]
- 目标 : aya 启动期能自动 load 用户机器对应的 BTF（即便内核没暴露）
- 涉及（新建）:
  - `xtask/src/btf_bundle.rs`（替换 M0 stub）
  - `resources/btf/.gitattributes`（LFS）
  - `src/collectors/ebpf/btf_resolver.rs`
- 修改 :
  - xtask 命令 fetch btfhub-archive，按 `--distros` 过滤（默认 centos7,8 / ubuntu18,20,22 / debian10,11）
  - 输出到 `resources/btf/<distro>-<ver>-<kernel>.btf`
  - 启动期：`/etc/os-release` 匹配 → 取出 BTF 写临时文件 → aya `BpfLoader::btf_path(...)`
- 验收 :
  - `cargo xtask btf-bundle --distros ubuntu22` 产物 ≤ 10MB
  - 在故意删掉 `/sys/kernel/btf/vmlinux` 的环境下，agent 仍能起
- 依赖 : T4.1

### 5.2 Week 10：Pixie TLS（aya 实现）

#### T4.6 OpenSSL uprobe + syscall kprobe 关联（aya 版）  [W10.D1 – W10.D2]  [2d]  [ ]
- 目标 : Pixie 调用栈关联法，OpenSSL 1.1.1 / 3.0 拿到明文
- 涉及（新建）:
  - `crates/zerotrace-ebpf-kernel/src/tls/openssl.rs` — 4 个 `#[uprobe]` / `#[uretprobe]`：`SSL_write` / `SSL_read`
  - `crates/zerotrace-ebpf-kernel/src/tls/syscall.rs` — 6 个 `#[kprobe]`：`write` / `sendto` / `sendmsg` / `read` / `recvfrom` / `recvmsg`
  - `crates/zerotrace-ebpf-kernel/src/tls/maps.rs` — `HashMap<tid, (buf_ptr, len, direction)>` 作为 tls_active map
  - `src/collectors/ebpf/tls/openssl_attacher.rs` — 扫 `/proc/<pid>/maps` 找 `libssl.so*` 路径并 attach
- 修改 :
  - uprobe entry: 写入 `tls_active[tid] = (buf, len, write)`；
  - syscall kprobe: 查 tls_active，命中则把 `(fd, plaintext_buf, plaintext_len)` 推 ringbuf；
  - uretprobe: 清理 map 项；完整性 metric `tls_correlation_success_total` / `_failed_total`
- 验收 :
  - `curl https://example.com` 在 agent log 看到明文 GET 请求
  - OpenSSL 1.1.1 + 3.0 双版本通；`tls_correlation_success_total / (success + failed) ≥ 0.99`
- 依赖 : T4.4（需要 syscall hook 与 sock 映射）

#### T4.7 BoringSSL 自动适配  [W10.D3]  [1d]  [ ]
- 目标 : 无需任何额外代码就让 Go gRPC（用 BoringSSL）也能解析
- 涉及（修改）: `src/collectors/ebpf/tls/openssl_attacher.rs`（扩展扫描逻辑）
- 修改 :
  - attacher 不仅扫 libssl.so*，也扫静态链接二进制内部的 `SSL_write` / `SSL_read` 符号（用 `object` crate 解析 ELF symtab）
  - 同一 BPF program 复用（BoringSSL ABI 兼容）
- 验收 :
  - 跑 `grpcurl` 客户端调用 HTTPS gRPC 服务，能解析出 method + 明文 body
- 依赖 : T4.6

#### T4.8 Go crypto/tls 单独处理  [W10.D4]  [1d]  [ ]
- 目标 : Go 原生 crypto/tls（不用 OpenSSL，Pixie 法不适用）
- 涉及（新建）:
  - `crates/zerotrace-ebpf-kernel/src/tls/go_tls.rs` — uprobe `crypto/tls.(*Conn).Read` / `Write`，按 Go ABI（参数走栈）拿明文 buf
  - `src/collectors/ebpf/tls/go_attacher.rs`
  - `resources/tls_offsets/go.json` — Go 1.21 / 1.22 / 1.23 三版本栈偏移表（来自 DWARF 或离线探测）
- 修改 :
  - 启动期 `go version <binary>` 拿到版本 → 选对应偏移；
  - kernel-side 用 shim 读 `goroutine` 上下文若需要
- 验收 :
  - Go 1.22 写的 HTTPS server 端，agent 能解析出客户端的明文 path
- 依赖 : T4.6

#### T4.9 TLS Source 装配 + 验收  [W10.D5]  [1d]  [ ]
- 目标 : 用户态 `TlsSource` 自动按进程类型挂对应 uprobe，对外只暴露一个 Source
- 涉及（新建）: `src/collectors/ebpf/tls/source.rs`
- 修改 :
  - 扫所有目标进程的 `/proc/<pid>/maps`，按规则路由：含 libssl.so → openssl_attacher；含 Go runtime symbol → go_attacher；含 BoringSSL 静态符号 → openssl_attacher（复用）
  - 实现 `Source<Output = TlsPayload> + Lifecycle`
- 验收 :
  - 跑 3 类客户端（curl / grpcurl / go-http-client）+ 任意 HTTPS server，全部能在用户态 Pipeline 拿到明文 batch
  - `cargo nextest run -p zerotrace-ebpf-kernel tls` 与 `cargo nextest run --bin zerotrace-agent tls` 全过
- 依赖 : T4.7, T4.8

### 5.3 Week 11：UniformSender 工厂化 + ConfigHandler 局部拆解

#### T4.10 UnifiedSenderFactory  [W11.D1 – W11.D2]  [2d]  [ ]
- 涉及（新建）: `src/reporters/factory.rs`
- 修改 : `factory.spawn::<T>(world, name, size, encoder)` 一次性完成队列创建 + stats 注册 + reporter task 启动
- 验收 : 单测 + 替换 1 处旧 `queue::bounded_with_debug` 调用，行为等价

#### T4.11 批量替换 30 处队列样板  [W11.D3 – W11.D4]  [2d]  [ ]
- 涉及（修改）: `src/trident.rs` 中 30 处队列样板（用 `grep -n "queue::bounded_with_debug" src/trident.rs` 定位）
- 修改 : 每处由 ≈15 行样板缩为 1 行 `factory.spawn::<T>(...)`
- 验收 :
  - `src/trident.rs` 行数较改造前减少 ≥ 300 行
  - `cargo nextest run` 全过；现有 sender 数据流不变

#### T4.12 ConfigHandler 局部拆解（仅 inputs 子树）  [W11.D5]  [1d]  [ ]
- 涉及（修改）: `src/config/handler.rs`（重点 `inputs.cbpf` / `inputs.ebpf` 两子树的 diff）
- 涉及（新建）:
  - `crates/zerotrace-config/src/bus.rs` — `ConfigBus` 实现（与 kernel 的 trait 配套）
- 修改 : 该子树原 ≈150 个 if-else diff → 通过 ConfigBus 派发；其余子树 M4 后再处理
- 验收 : 改 inputs.cbpf.af_packet.bpf_filter_disabled 配置 → 对应 subscriber 收到事件并重启 dispatcher

### 5.4 Week 12：Server 算法模块

#### T4.13 Trace 组装算法  [W12.D1 – W12.D2]  [2d]  [ ]
- 涉及（新建）:
  - `../zerotrace-server/server/controller/algorithm/trace_assembler.go`
  - `../zerotrace-server/server/controller/db/clickhouse/migration/2026120101_traces.sql`
- 修改 :
  - 消费 ingester 写入的 span，按 trace_id 5 分钟滑动窗口聚合
  - 输出 `traces` 表（trace_id 主键，spans JSONB / Array(JSON)）
  - 缺 parent 时按五元组反推（沿用 DeepFlow 思路）
- 验收 : 注入 10 条带 trace_id 的 span，`SELECT * FROM traces` 能看到聚合

#### T4.14 异常检测算法 v1（EWMA + Robust Z-score）  [W12.D3]  [1d]  [ ]
- 涉及（新建）:
  - `../zerotrace-server/server/controller/algorithm/anomaly_ewma.go`
  - `../zerotrace-server/server/controller/db/mysql/migration/2026120301_anomalies.sql`
- 修改 :
  - 每分钟定时任务扫 `metrics` 表（最近 30 分钟窗口）
  - 算 z-score = |x - μ| / (1.4826 * MAD)；> 3.0 → 写 `anomalies(agent_id, metric, start_ts, end_ts, score)`
- 验收 :
  - 用 Yahoo S5 数据集回放，F1 ≥ 0.7
  - server 内置 demo：注入 CPU 飙高 10 分钟，能在 anomalies 表看到记录

#### T4.15 REST API for future frontend  [W12.D4]  [1d]  [ ]
- 涉及（新建）:
  - `../zerotrace-server/server/controller/http/handler/zerotrace/v1/query.go`
  - `docs/api/openapi.yaml`
- 修改 : 5 个查询端点（agents / metrics / traces / anomalies / spans-of-trace）+ OpenAPI 3.0 spec
- 验收 : `curl` 验证每个端点；`swagger-cli validate docs/api/openapi.yaml` 通过

#### T4.16 M3 收尾  [W12.D5]  [1d]  [ ]
- 验收 :
  - §10.4 M3 验收指标全 ✓
  - `git tag m3-done`；复盘 200 字附文末

---

## 6. M4：部署、验证、收尾（W13–W16，20 工作日）

> **里程碑目标**：可发布的公测版本——一键部署脚本、多 OS 兼容、一周长跑、demo。

### 6.1 Week 13：部署脚本

#### T5.1 Agent release 流水线  [W13.D1 – W13.D2]  [2d]  [ ]
- 涉及（新建）:
  - `install.sh` — 一键安装脚本
  - `xtask/src/release.rs` — 实现 M0 留的 stub
  - `manifests/daemonset.yaml`
  - `docker/Dockerfile.agent`
- 修改 :
  - xtask release 子命令：`cargo build --release --target=<target>`、strip、tar、生成 sha256
  - install.sh：按 `uname -r` 选择 libc/musl 产物；写 `/etc/zerotrace-agent.yaml`；systemd unit 安装
- 验收 :
  - `cargo xtask release --target x86_64-unknown-linux-gnu` 产物存在 `dist/`
  - 在干净 Ubuntu 22.04 VM 上 `curl install.sh | bash` 一键成功

#### T5.2 Server 容器化  [W13.D3]  [1d]  [ ]
- 涉及（新建）:
  - `docker/Dockerfile.server`
  - `docker-compose.yml`（含 server + MySQL + ClickHouse + Redis；剔除 DeepFlow `deepflow-app`）
- 验收 : `docker compose up -d` 后所有 service healthy

#### T5.3 端到端联调脚本  [W13.D4]  [1d]  [ ]
- 涉及（新建）: `scripts/e2e_smoke.sh`
- 修改 : 启 server + agent，跑 5 分钟，检查 ClickHouse 至少 1000 行 metric + 100 条 span
- 验收 : 脚本 exit 0

#### T5.4 文档刷新  [W13.D5]  [1d]  [ ]
- 涉及（修改）: `README.md` / `docs/install.md`（新）
- 验收 : 自查、外部人员按文档安装能成功

### 6.2 Week 14：多 OS 矩阵验证

#### T5.5 Ubuntu 22.04（5.15）  [W14.D1]  [1d]  [ ]
- 验收 : 全功能（metric + trace + TLS 解密）

#### T5.6 Ubuntu 20.04（5.4）  [W14.D2]  [1d]  [ ]
- 验收 : 全功能

#### T5.7 CentOS Stream 8（4.18）  [W14.D3]  [1d]  [ ]
- 验收 : 全功能 + 验证 aya socket_trace 子集在 4.18 也能 load（btfhub BTF 兜底）

#### T5.8 CentOS 7.9（3.10）  [W14.D4]  [1d]  [ ]
- 验收 : metric 全功能；trace 给出明确"内核版本不支持"日志（非 crash）

#### T5.9 Debian 11（5.10）  [W14.D5]  [1d]  [ ]
- 验收 : 全功能；产出 `docs/COMPATIBILITY.md` 总览

### 6.3 Week 15：长跑稳定性 + 数据准确性

#### T5.10 测试床搭建  [W15.D1]  [1d]  [ ]
- 涉及（新建）: `tests/e2e/longrun/`
- 修改 : 3 台 VM + opentelemetry-demo + Prometheus 监控 agent 自身 RSS / CPU
- 验收 : 监控 dashboard 可见

#### T5.11 一周长跑  [W15.D2 – W15.D7]  [5d，实际背景跑]  [ ]
- 验收 :
  - 7 天无 crash
  - RSS 稳定 < 300MB
  - 上报丢包率 < 0.1%
  - 异常检测算法在 anomaly demo 中有正确告警

### 6.4 Week 16：Demo + 收尾

#### T5.12 Demo 1：定位异常 trace  [W16.D1]  [1d]  [ ]
- 涉及（新建）: `examples/demo/slow-sql/`
- 修改 : MySQL 注入慢查询 → 录屏 / asciinema：从 anomalies 表起跳到具体 trace
- 验收 : 视频 ≤ 5 分钟，能在没有人讲解的情况下复现

#### T5.13 Demo 2：定位异常主机  [W16.D2]  [1d]  [ ]
- 涉及 : `examples/demo/cpu-spike/`
- 修改 : `stress-ng --cpu 8 --timeout 600s` 制造异常 → 录屏从异常列表跳到 host metric 时序图（命令行渲染）
- 验收 : 同上

#### T5.14 扩展性验证 demo  [W16.D3]  [1d]  [ ]
- 涉及（新建）: `examples/extension/dummy-ai-processor/`
- 修改 : 在 `src/processors/dummy_ai.rs` 写一个 Processor（不实现真算法，输出固定 Custom 信号），在 `src/bundles/ai.rs` 注册，YAML pipeline 引用即生效
- 验收 : 不改任何已有 crate 一行代码就能装载（git diff 仅含新文件）

#### T5.15 文档刷新  [W16.D4]  [1d]  [ ]
- 涉及（新建/修改）:
  - `docs/ARCHITECTURE.md` — 替换原 DeepFlow ARCHITECTURE_EVOLUTION.md，按 `draft.md` 内容落地
  - `docs/USER_GUIDE.md` — 配置项 / pipeline DSL / 故障排查
  - `docs/OPERATOR_GUIDE.md` — server 部署 / API_KEY 管理 / 升级
- 验收 : 三份文档对应章节齐全；按文档可由不熟悉项目的人独立完成部署

#### T5.16 v0.1.0-rc1 发布  [W16.D5]  [1d]  [ ]
- 验收 :
  - GitHub Release 页面有 tag、release notes、3 个二进制（x86_64-gnu/musl + aarch64）
  - 复盘 200 字
  - `git tag v0.1.0-rc1`

---

## 7. 持续任务（贯穿 M0–M4）

每周末执行一次，不计入里程碑 work item，但若漏做下周第一件事补：

- [ ] **C1** 跑 `cargo xtask check`，失败立刻修
- [ ] **C2** 跑 `cargo hakari generate --diff` 并提交（仅 M0 之后）
- [ ] **C3** 跑 `cargo machete` 找未用依赖，删之
- [ ] **C4** 跑 `typos` 修拼写
- [ ] **C5** review 本周新增 unsafe 代码（应只在 source-ebpf / source-packet 出现）
- [ ] **C6** 抽 30 分钟更新本文档：勾完成项 / 补漏项 / 调时间

---

## 8. 风险登记

| 风险 | 概率 | 影响 | 缓解 | 触发应对 |
|---|---|---|---|---|
| aya + shim 在某老内核 ABI 不兼容 | 中 | 高 | T4.5 备 btfhub 集；shim 字段访问失败可降级到本地 `bpf_probe_read_kernel` | 该内核降级到 metric only |
| aya 0.13 版本 API break | 低 | 中 | Cargo.lock 锁定，跟踪 changelog | 出 issue 升 0.14 时单独评估 1 天工作量 |
| C shim 字段表漂移成本 | 低 | 低 | 新增字段一行 SHIM(struct, member)；CI 跑 `cargo build` 即触发 bindgen 重生成 | 出错时回滚字段表 |
| Pixie 调用栈关联在多线程 syscall 失效 | 中 | 中 | 保留 DeepFlow 旧 SSL_write 偏移表作降级 | 切回旧路径，记 issue |
| Server fork 与 DeepFlow upstream 大 merge 冲突 | 高 | 中 | 改动集中在新建包 | 冲突时 cherry-pick 关键 fix |
| trident.rs 重构破坏现有数据面 | 中 | 极高 | 新旧并存策略（draft.md §10.1） | 立刻回滚 feature flag |
| 4 个月不够 | 中 | 中 | M3 W11 可缩水到只拆 inputs 子树；M4 多 OS 矩阵可只 3 个 | 砍 M3 收尾里的"ConfigHandler 全拆" |
| 长跑暴露内存泄漏 | 中 | 高 | W15 早启动 | 用 heaptrack 找泄漏点 |
| 第三方依赖安全漏洞（cargo deny 告警） | 中 | 中 | C1 每周跑 | 按 advisory 提 PR 升级 |

---

## 9. 验收指标速查

### 10.1 M0（W1–W2）
- [ ] `crates/zerotrace-core` + `kernel` + `runtime` + `config` 编译通过
- [ ] World/SystemParam/Lifecycle/Bundle 4 个 trait 单测覆盖 ≥ 80%
- [ ] 端到端 spike：proc CPU → stdout 输出 JSON metric
- [ ] `cargo xtask check` 全绿
- [ ] tag `m0-done`

### 10.2 M1（W3–W4）
- [ ] MetricServer 拆为 5 个独立 Receiver Component
- [ ] HTTP 短连接 6 端点工作；API_KEY 鉴权链路通
- [ ] 不带 key 请求被 401
- [ ] 老 gRPC stream 代码标 `#[deprecated]`，仍可编译
- [ ] tag `m1-done`

### 10.3 M2（W5–W8）
- [ ] 主机指标 178 项全部正确（fixture 单测全过）
- [ ] 包重组 + 乱序重排实现并接入 microservice pipeline
- [ ] 注入分片 HTTP/2 流量能在 l7 parser 拿到完整帧
- [ ] cloud-platform Bundle 装载 IMDS + K8s + libvirt + process 4 个 tagger
- [ ] tag `m2-done`

### 10.4 M3（W9–W12）
- [ ] aya + shim 在 ≥ 3 个内核版本（4.18 / 5.4 / 5.15）load 成功并产出 socket_event
- [ ] aya 路径与现有 C 路径**并存**，老 probe（perf_profiler 等）继续工作不退化
- [ ] Pixie TLS 解析 OpenSSL 3.0 + BoringSSL + Go crypto/tls 三类都拿到明文
- [ ] `src/trident.rs` < 3000 行（当前 4123）
- [ ] server `traces` 表能聚合，`anomalies` 表能输出异常区间
- [ ] OpenAPI spec 通过 swagger-cli validate
- [ ] tag `m3-done`

### 10.5 M4（W13–W16）
- [ ] 5 个 OS 全跑（CentOS 7.9 仅 metric，其余全功能）
- [ ] 一周长跑 agent 无 crash、RSS < 300MB
- [ ] 2 个 demo 视频可重放
- [ ] 扩展性 demo：加新 AI bundle 不改任何已有 crate
- [ ] tag `v0.1.0-rc1`

---

## 10. 阶段复盘（每个 milestone 结束追加）

### M0 复盘（W2 末填）
*待填，限 200 字。模板：成功之处 / 教训 / 下一阶段调整。*

### M1 复盘（W4 末填）
*待填*

### M2 复盘（W8 末填）
*待填*

### M3 复盘（W12 末填）
*待填*

### M4 复盘（W16 末填）
*待填*

---

*文档版本：todo v1，与 `draft.md v1` 配套*
*若架构发生变更，先改 `draft.md`，再回头同步本文件的任务表*
