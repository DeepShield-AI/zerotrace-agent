# Agent 采集 K8s 数据配置指南（宿主模式）

## 1. 架构总览

```
K8s 节点（worker1/worker2）
  └─ zerotrace-agent（宿主进程，每节点一个）
       ├─ 流量采集（AF_PACKET）──→ server:30033（数据面）
       ├─ genesis（主机接口/IP）──→ server（控制面）
       └─ K8s watcher（API server）──→ 上报 pod/node 等资源 ──→ server ──→ ClickHouse
                 │
                 └─ 发现 API server：kubeconfig（~/.kube/config）
```

## 2. Agent 配置

### 2.1 配置文件（zerotrace-agent.yaml）

```yaml
# ── 与 server 的连接（两个节点相同）──
ingester_ip:                      # 数据面（流量上报）
  - <server_ip>
proxy_controller_ip:              # 控面（sync）
  - <server_ip>
proxy_controller_port:
  - 30035
analyzer_ip:
  - <server_ip>
controller-ips:
  - <server_ip>
controller-port: 30035

# ── K8s 同步（关键）──
kubernetes-cluster-id: d-k8s01abcde   # 必须配置

# ── 其他 ──
ebpf:
  profile:
    on_cpu:
      disabled: true
```

### 2.2 启动环境变量（关键）

| 变量 | 值 | 作用 |
|------|-----|------|
| `K8S_WATCH_POLICY` | `watch-only` | **强制启用 K8s watcher**（不依赖 server 下发开关），宿主模式必须 |
| `ZT_DATA_VIA_HTTP` | `false` | 数据面走 gRPC（默认） |
| `RUST_LOG` | `info` | 日志级别 |

### 2.3 启动命令

```bash
cd /root && \
K8S_WATCH_POLICY=watch-only ZT_DATA_VIA_HTTP=false RUST_LOG=info \
setsid nohup ./zerotrace-agent -f zerotrace-agent.yaml > /tmp/agent_start.log 2>&1 < /dev/null &
```

### 2.4 依赖文件

- **kubeconfig**：`/root/.kube/config`（agent 通过 `Config::infer()` 发现 API server；
  可从 K8s 的 `admin.conf` 拷贝，需能访问 `https://<api-server>:6443`）
- **root 权限**：抓包需要

## 3. kubernetes-cluster-id 的作用

**关键配置**：不配置则 server 的 K8s 开关条件（`clusterID != ""`）不满足，
watcher 不启动，K8s 数据为 0（流量/主机采集不受影响）。

机制：
1. agent 在 SyncRequest 中上报 cluster_id（proto 字段 45）
2. server 校验 `clusterID != ""` 且 watch_policy != disabled 且 vtap 类型在白名单
   → 下发 `kubernetes_api_enabled=true`
3. agent 收到 true → trident.rs:1104 → `api_watcher.start()` → 连接 API server 采集
4. agent 上报的 K8s 资源带 cluster_id → server 关联到 kubernetes domain

注意事项：
- 值可自定义（`d-` 开头），agent 与 server 对上即可
- server 表无记录时自动注册（无需手工）
- 多个 agent 同一 cluster-id：只有一个成为 owner（kubernetes_cluster 表 value）；
  watch-only 模式下其他 agent 会抢（force 更新），都能工作

## 4. 验证

```bash
# 1. agent 侧：watcher 运行
grep -E 'api watcher|api server url' /var/log/zerotrace-agent/zerotrace-agent_rCURRENT.log | tail -3
# 期望：kubernetes api watcher running / api server url is: https://<api-server>:6443/

# 2. server 侧：K8s 开关下发
grep 'open cluster' /var/log/deepflow/server.log | tail -3

# 3. server 侧：K8s 数据持续上报（K8sVersion 增长、K8sLastSeen 更新）
curl -s 'http://127.0.0.1:20417/v1/agent-stats/1/' -H 'X-User-Id: 1'

# 4. ClickHouse：pod 资源表（tagrecorder 生成，约 10-30 分钟）
curl -s 'http://<clickhouse>:8123/?query=SELECT%20count()%20FROM%20flow_tag.ch_pod'
```