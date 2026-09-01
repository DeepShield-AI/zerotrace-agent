# ZeroTrace Agent 编译指南

本文档介绍如何使用 Docker 编译 `zerotrace-agent` 和 `zerotrace-agent-ctl`，无需配置本地 Rust 环境。

Windows 安装、Npcap 依赖、配置和运行方式请参阅 [Windows 部署指南](./windows-deployment.md)。

## 1. 安装 Docker

如果系统尚未安装 Docker，请按以下步骤安装。

### Ubuntu / Debian

```bash
# 更新包索引
sudo apt-get update

# 安装依赖包
sudo apt-get install -y \
    ca-certificates \
    curl \
    gnupg \
    lsb-release

# 添加 Docker 官方 GPG 密钥
sudo install -m 0755 -d /etc/apt/keyrings
curl -fsSL https://download.docker.com/linux/ubuntu/gpg | sudo gpg --dearmor -o /etc/apt/keyrings/docker.gpg
sudo chmod a+r /etc/apt/keyrings/docker.gpg

# 添加 Docker APT 源
echo \
  "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://download.docker.com/linux/ubuntu \
  $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# 安装 Docker Engine
sudo apt-get update
sudo apt-get install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
```

### CentOS / RHEL / Fedora

```bash
# 安装 yum-utils
sudo yum install -y yum-utils

# 添加 Docker 仓库
sudo yum-config-manager --add-repo https://download.docker.com/linux/centos/docker-ce.repo

# 安装 Docker Engine
sudo yum install -y docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin
```

### 国内镜像源（可选，网络不通时使用）

如果无法访问 Docker 官方源，可替换为阿里云镜像：

```bash
# Ubuntu
echo \
  "deb [arch=$(dpkg --print-architecture) signed-by=/etc/apt/keyrings/docker.gpg] https://mirrors.aliyun.com/docker-ce/linux/ubuntu \
  $(. /etc/os-release && echo "$VERSION_CODENAME") stable" | \
  sudo tee /etc/apt/sources.list.d/docker.list > /dev/null

# CentOS
sudo yum-config-manager --add-repo https://mirrors.aliyun.com/docker-ce/linux/centos/docker-ce.repo
```

### 启动 Docker

```bash
sudo systemctl start docker
```

### 验证安装

```bash
# 查看版本
docker --version

# 运行测试容器
sudo docker run --rm hello-world
```

### （可选）免 sudo 使用 Docker

```bash
# 将当前用户加入 docker 组
sudo usermod -aG docker $USER

# 重新登录后生效，或执行：
newgrp docker
```

## 2. 配置 Docker (支持私有镜像仓库)

编译镜像位于私有仓库（HTTP 协议），需要修改 Docker 配置以允许不安全的镜像库。

修改 `/etc/docker/daemon.json`:
```json
{
  "insecure-registries": ["47.97.67.233:5000"]
}
```

重启 Docker 服务以生效：
```bash
sudo systemctl daemon-reload
sudo systemctl restart docker
```

## 3. 获取代码

拉取代码时需要包含子模块：

```bash
git clone --recurse-submodules https://github.com/DeepShield-AI/zerotrace-agent.git
cd zerotrace-agent
```

## 4. 编译项目

### 4.1 Debug 编译

```bash
docker run --privileged --rm -it \
    -v $(pwd):/zerotrace \
    47.97.67.233:5000/deepshield/zerotrace-builder \
    bash -c "cd /zerotrace && cargo build"
```

**首次编译**：仅编译项目代码（依赖已预编译在镜像中），约 2-5 分钟。
**后续编译**：增量编译，仅编译变更文件，通常秒级完成。

### 4.2 编译产物

| 文件 | 路径 |
|------|------|
| Agent | `target/debug/zerotrace-agent` |
| Ctl | `target/debug/zerotrace-agent-ctl` |

### 4.3 Release 编译

```bash
docker run --privileged --rm -it \
    -v $(pwd):/zerotrace \
    47.97.67.233:5000/deepshield/zerotrace-builder \
    bash -c "cd /zerotrace && cargo build --release"
```

产物路径：`target/release/zerotrace-agent`、`target/release/zerotrace-agent-ctl`

### 4.4 编译 Windows x64 版本

Windows 版本使用 GNU 目标进行交叉编译。以下命令在 Ubuntu/Debian Linux 主机上执行：

```bash
# 安装 MinGW 工具链
sudo apt-get update
sudo apt-get install -y \
  gcc-mingw-w64-x86-64 \
  binutils-mingw-w64-x86-64 \
  unzip

# 安装 Rust Windows GNU 目标
rustup target add x86_64-pc-windows-gnu

# 下载并解压 Npcap SDK（提供 wpcap.lib）
curl -L -o /tmp/npcap-sdk.zip https://npcap.com/dist/npcap-sdk-1.15.zip
mkdir -p /tmp/npcap-sdk
unzip -q /tmp/npcap-sdk.zip -d /tmp/npcap-sdk

# 编译（需要 Npcap SDK 的 x64 导入库）
export NpcapSdk=/tmp/npcap-sdk
export CARGO_TARGET_DIR=/tmp/zerotrace-target
export RUSTFLAGS="-C force-frame-pointers=yes -L native=$NpcapSdk/Lib/x64"
export LIBPCAP_VER=1.10.6
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc-posix

cargo build --release \
  --target x86_64-pc-windows-gnu \
  --no-default-features

mkdir -p dist/windows
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/zerotrace-agent.exe" dist/windows/
cp "$CARGO_TARGET_DIR/x86_64-pc-windows-gnu/release/zerotrace-agent-ctl.exe" dist/windows/
cp config/zerotrace-agent-windows.yaml dist/windows/zerotrace-agent.yaml
```

Windows 运行时必须安装 Npcap，并确保 `wpcap.dll` 和 `Packet.dll` 可被 Agent 加载。

## 5. 编译参数说明

| 参数 | 说明 |
|------|------|
| `--privileged` | 赋予容器特权，eBPF 相关编译可能需要 |
| `--rm` | 容器退出后自动删除 |
| `-it` | 交互模式，可看到编译进度 |
| `-v $(pwd):/zerotrace` | 将项目源码挂载到容器 |
| `-v .../target` | 持久化编译产物，实现增量编译 |
| `47.97.67.233:5000/deepshield/zerotrace-builder` | 自建编译镜像（内含 Rust 1.96.0 + 预编译依赖） |

### 缓存机制说明

```
镜像构建时（一次性）:
  依赖 crates → [下载] → [编译] → 镜像层缓存  (慢，仅一次)

日常编译时:
  源码 → [项目代码编译] → target/debug/  (快，秒级)
              ↑ 仅编译有改动的文件
```

## 6. 运行 Agent

编译完成后，需配置 Agent 连接 ZeroTrace Server。

### 6.1  创建配置文件

修改 `config/zerotrace-agent.yaml`，填入以下内容（<zerotrace-server ip>替换为真实ip）：

```yaml
## ingester addresses
ingester_ip:
  - <zerotrace-server ip>

## proxy controller
proxy_controller_ip:
  - <zerotrace-server ip>

proxy_controller_port:
  - 30035

## analyzer
analyzer_ip:
  - <zerotrace-server ip>

## controller ip
controller-ips:
  - <zerotrace-server ip>

## controller listen port
controller-port: 30035

## logfile path
log-file: /opt/zerotrace-agent/var/log/zerotrace-agent.log

ebpf:
  profile:
    on_cpu:
      disabled: true
```

### 6.2 启动 Agent

```bash
# 杀死可能存在的旧进程
sudo pkill zerotrace-agent 2>/dev/null || true

# 以 Managed 模式启动 Agent
sudo ZT_DATA_VIA_HTTP=false RUST_LOG=info ./target/debug/zerotrace-agent \
  -f config/zerotrace-agent.yaml > logs/test.log 2>&1

# 启动 k8s 数据采集
# 需先根据 k8s采集配置文件 (./k8s-collection.md) 修改配置
sudo K8S_WATCH_POLICY=watch-only ZT_DATA_VIA_HTTP=false RUST_LOG=info \
 ./target/debug/zerotrace-agent -f config/zerotrace-agent.yaml > logs/test.log 2>&1

# 查看日志确认启动成功
tail -f logs/test.log
```

> **注意**：Agent 需要 root 权限运行（eBPF 采集需要），并且必须通过 `-f` 参数指定配置文件路径。

### 6.3 验证运行状态

```bash
# 检查进程是否在运行
ps aux | grep zerotrace-agent

# 用 ctl 工具检查 Agent 状态（调试端口固定为 30033）
./target/debug/zerotrace-agent-ctl -p 30033 cpu show
```

## 7. 常见问题

### 7.1 Docker 服务启动失败：`process with PID XXXX is still running`

残留的 dockerd 进程占用 pid 文件导致 systemd 无法启动新 daemon：

```bash
# 查看残留进程
ps aux | grep dockerd

# 强制终止残留进程
sudo kill -9 <PID>

# 删除旧的 pid 文件
sudo rm -f /var/run/docker.pid

# 重新启动
sudo systemctl start docker
```

### 7.2 执行编译命令后无任何输出

可能原因及排查步骤：

**a) Docker 服务未运行**
```bash
sudo systemctl status docker
# 如果未运行，启动它：
sudo systemctl start docker
```

**b) 私有镜像仓库不可达**
```bash
# 测试是否能拉取镜像
docker pull 47.97.67.233:5000/deepshield/rust-build:cached
```

如果网络不通，请确认：
- 已按第 2 节配置了 `insecure-registries`
- `47.97.67.233:5000` 在内网可达（ping 测试）
- 防火墙未阻止 5000 端口

**c) 首次编译耗时较长**

首次 `cargo build` 需要下载所有依赖 crate，可能几分钟无屏幕输出。可以加 `-v` 查看 cargo 详细输出：
```bash
docker run --privileged --rm -it \
    -v $(pwd):/zerotrace \
    -v $(pwd)/.docker-cache/target:/zerotrace/target \
    47.97.67.233:5000/deepshield/zerotrace-builder \
    bash -c "cd /zerotrace && cargo build -v"
```

### 7.3 权限不足

如果遇到 permission denied：
```bash
# 确保当前用户在 docker 组
sudo usermod -aG docker $USER
newgrp docker
```
