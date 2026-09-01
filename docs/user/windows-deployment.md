# Windows 部署指南

本文介绍如何在 Windows 主机上安装、配置和运行 `zerotrace-agent`。

> Windows 版本的 Agent 使用 Npcap/libpcap 抓取 Windows 网卡流量，不使用 Linux
> 的 AF_PACKET、eBPF、Linux network namespace 或 cgroups 功能。

## 1. 适用范围

当前 Windows 构建目标为 **x86_64 Windows**。推荐环境：

- Windows 10/11 64 位；
- 具有本机管理员权限；
- 已安装 Npcap；
- Agent 所在主机可以访问控制器和数据接收端口；
- 使用与 Agent 构建版本匹配的 `zerotrace-agent.exe`。

Windows Agent 适合采集 Windows 主机网卡上实际可见的流量。它不能直接访问
Docker Desktop、WSL2 或 Hyper-V Linux VM 内部的 `br-*`、`veth*` 等 Linux 网卡。
容器到容器的流量如果没有经过 Windows 主机网卡，Windows Npcap 也无法抓到。

## 2. 运行依赖

### 2.1 Npcap

Windows Agent 通过 libpcap 调用 Npcap 的抓包接口。运行时需要安装 Npcap，不能只
下载 Agent 可执行文件。

1. 从 Npcap 官方网站下载 Windows 安装程序：
   <https://npcap.com/#download>
2. 以管理员身份运行安装程序；
3. 建议勾选 **Install Npcap in WinPcap API-compatible Mode**；
4. 按需选择 **Install Npcap in WinPcap API-compatible Mode** 之外的默认选项，
   完成安装并按提示重启。

验证 Npcap 服务：

```powershell
Get-Service npcap -ErrorAction SilentlyContinue
Get-Service | Where-Object { $_.Name -match 'npcap|npf' }
```

验证 Npcap 的 DLL（路径因安装版本可能不同）：

```powershell
Get-ChildItem `
  "$env:WINDIR\System32\Npcap\wpcap.dll", `
  "$env:WINDIR\System32\Npcap\Packet.dll" `
  -ErrorAction SilentlyContinue | Select-Object FullName
```

如果找不到 `wpcap.dll` 或 `Packet.dll`，请重新安装 Npcap 并启用 WinPcap 兼容模式。
不要从其他机器随意复制 DLL，避免 DLL 与 Npcap 驱动版本不匹配。

### 2.2 网络和防火墙

至少允许 Agent 到控制面和数据面的出站 TCP 连接。端口以控制器下发配置为准，常见
端口如下：

| 用途 | 默认端口 |
|---|---:|
| Controller gRPC | `30035` |
| Ingester 数据上报（gRPC/TCP） | `30033` |
| Controller TLS（可选） | `30135` |

可以使用 PowerShell 测试连通性：

```powershell
Test-NetConnection <controller-ip> -Port 30035
Test-NetConnection <ingester-ip> -Port 30033
```

## 3. 安装目录和文件

建议将 Agent 安装到不包含空格的目录，例如：

```text
C:\ZeroTrace\Agent\
├── zerotrace-agent.exe
├── zerotrace-agent-ctl.exe       # 可选，用于本地诊断
├── zerotrace-agent.yaml
└── log\
```

## 4. 配置 Agent

### 4.1 最小托管配置

Windows Agent 默认使用托管模式：启动时连接 Controller，其他采集配置由
Controller/Agent Group 下发。

编辑 `zerotrace-agent.yaml`：

```yaml
# Controller 地址
controller-ips:
  - <controller-ip>

# Controller gRPC 端口
controller-port: 30035

# Windows 路径必须使用 Windows 格式。
# YAML 使用单引号时，反斜杠不需要再次转义。
log-file: 'C:\ProgramData\ZeroTrace\Agent\log\zerotrace-agent.log'

# 默认值为 ip-and-mac；如果环境中的 MAC 地址不稳定，可以改为 ip。
agent-unique-identifier: ip-and-mac
```

建议先创建日志目录：

```powershell
New-Item -ItemType Directory -Force `
  C:\ProgramData\ZeroTrace\Agent\log | Out-Null
```

`controller-ips` 是 Windows Agent 首次注册所需的关键配置。控制器、Ingester 和
Analyzer 的最终地址通常会在托管模式同步阶段下发；如果部署环境使用旧版静态配置，
则应按照对应版本的配置模板同时填写数据面地址。

### 4.3 Windows 网卡采集配置

托管模式下，采集配置通常通过 Agent Group 下发。Windows Agent 的关键配置路径为：

```yaml
inputs:
  cbpf:
    af_packet:
      interface_regex: '^(WLAN|以太网|vEthernet.*)$'
    special_network:
      libpcap:
        enabled: true
```

其中：

- `interface_regex` 按 Windows 网卡的友好名称匹配；
- Windows 默认使用 libpcap/Npcap，`libpcap.enabled` 应保持为 `true`；
- `vEthernet.*` 是否需要保留，取决于是否要采集经过 Hyper-V/WSL 边界的流量；
- 不要在 Windows 配置中加入 `br-*`、`veth*`、`eth*` 来尝试采集 Docker Desktop
  Linux VM 内部流量，这些接口不会出现在 Windows Npcap 的设备列表中。

查看本机网卡名称：

```powershell
Get-NetAdapter |
  Sort-Object ifIndex |
  Format-Table ifIndex, Name, InterfaceDescription, Status, MacAddress -AutoSize
```

例如只采集无线网卡和 WSL/Hyper-V 虚拟网卡：

```yaml
inputs:
  cbpf:
    af_packet:
      interface_regex: '^(WLAN|vEthernet.*)$'
```

如果 Windows 使用中文网卡名称，则应使用实际名称，例如：

```yaml
inputs:
  cbpf:
    af_packet:
      interface_regex: '^(WLAN|以太网|vEthernet.*)$'
```

如果只需要采集本机进程之间的回环流量，并且 Npcap 安装了 Loopback Adapter，
可以显式加入它；否则不建议默认采集回环接口，以免引入大量系统后台流量：

```yaml
inputs:
  cbpf:
    af_packet:
      interface_regex: '^(WLAN|vEthernet.*|Npcap Loopback Adapter)$'
```

`src_interfaces` 已经废弃，应优先使用 `interface_regex`。

### 4.4 排除 Agent 自身通信（可选）

如果不希望把 Agent 自身的控制和数据连接作为业务流量处理，可以在 Agent Group
配置中增加 BPF 过滤条件：

```yaml
inputs:
  cbpf:
    af_packet:
      extra_bpf_filter: 'not port 30035 and not port 30033'
```

请根据实际 Controller、Ingester 端口调整过滤条件。不要误过滤业务使用的端口。

### 4.5 配置生效方式

| 配置类型 | 来源 | 生效方式 |
|---|---|---|
| Controller 地址、TLS、日志路径 | 本地 YAML | 重启 Agent |
| 网卡正则、抓包和协议策略 | 托管配置/Agent Group | 由 Controller 同步，必要时重启 Agent |
| `ZT_DATA_VIA_HTTP`、`RUST_LOG` | 进程环境变量 | 重启 Agent |

配置文件中的路径必须对运行 Agent 的账户可写。作为 Windows 服务运行时，建议使用
`C:\ProgramData\ZeroTrace\Agent\log`，不要使用用户目录下的临时路径。

## 5. 前台运行和验证

### 5.1 查看版本

```powershell
Set-Location C:\ZeroTrace\Agent
.\zerotrace-agent.exe --version
.\zerotrace-agent.exe --help
```

Windows 构建的默认配置文件名是当前目录下的 `zerotrace-agent.yaml`，也可以显式
指定配置文件：

```powershell
.\zerotrace-agent.exe -f .\zerotrace-agent.yaml
# -c 是 -f 的别名
```

### 5.2 推荐的首次启动方式

首次部署时先以前台方式启动，便于直接看到 Npcap、配置和连接错误：

```powershell
Set-Location C:\ZeroTrace\Agent
$env:ZT_DATA_VIA_HTTP = 'false'
$env:RUST_LOG = 'info'
.\zerotrace-agent.exe -f .\zerotrace-agent.yaml
```

`ZT_DATA_VIA_HTTP=false` 表示数据面使用 gRPC/TCP 路径。关闭窗口或按 `Ctrl+C` 会
停止 Agent。

### 5.3 检查进程和日志

另开一个 PowerShell 窗口执行：

```powershell
Get-Process zerotrace-agent -ErrorAction SilentlyContinue
Get-Content C:\ProgramData\ZeroTrace\Agent\log\zerotrace-agent.log -Wait
```

如果配置使用相对日志路径，也可以检查安装目录下的日志文件：

```powershell
Get-ChildItem C:\ZeroTrace\Agent\log
```

启动日志应重点确认：

- Controller 连接成功并完成 Agent 注册；
- Npcap/libpcap 能打开至少一个匹配的网卡；
- Agent Group 配置已同步；
- 没有 `no capture interface`、`wpcap.dll` 或 `Packet.dll` 错误。

### 5.4 使用 Agent Ctl（可选）

如果发布包包含 `zerotrace-agent-ctl.exe`，可以查询本地 Agent 状态：

```powershell
Set-Location C:\ZeroTrace\Agent
.\zerotrace-agent-ctl.exe -p 30033 cpu show
```

如果 Ctl 版本与 Agent 不匹配，或本地调试端口被修改，请以 Agent 日志为准。

## 6. 注册为 Windows 服务

仓库当前提供 Agent 可执行文件和 Linux systemd 示例，但不提供 MSI 或原生 Windows
Service Installer。生产环境可以使用 WinSW、NSSM 或企业现有的软件分发系统托管
Agent。下面给出 WinSW 示例。

### 6.1 准备 WinSW

从 WinSW 官方 Release 下载 x64 版本：

<https://github.com/winsw/winsw/releases/latest/download/WinSW-x64.exe>

将文件保存为：

```text
C:\ZeroTrace\Agent\zerotrace-agent-service.exe
```

> WinSW 是第三方服务包装器，不属于 zerotrace-agent 发布物。下载后应按组织的
> 软件供应链要求校验文件来源和哈希。

### 6.2 创建服务配置

在同一目录创建 `zerotrace-agent-service.xml`：

```xml
<service>
  <id>ZeroTraceAgent</id>
  <name>ZeroTrace Agent</name>
  <description>ZeroTrace network observability agent</description>
  <executable>C:\ZeroTrace\Agent\zerotrace-agent.exe</executable>
  <arguments>-f "C:\ZeroTrace\Agent\zerotrace-agent.yaml"</arguments>
  <workingdirectory>C:\ZeroTrace\Agent</workingdirectory>
  <env name="ZT_DATA_VIA_HTTP" value="false" />
  <env name="RUST_LOG" value="info" />
  <log mode="roll" />
  <onfailure action="restart" delay="10 sec" />
  <stoptimeout>15sec</stoptimeout>
</service>
```

`log-file` 仍然由 YAML 配置控制；WinSW 的滚动日志主要用于收集服务包装器自身
的标准输出和错误输出。

### 6.3 安装、启动和停止服务

以管理员身份打开 PowerShell：

```powershell
Set-Location C:\ZeroTrace\Agent
.\zerotrace-agent-service.exe install
Start-Service ZeroTraceAgent
Get-Service ZeroTraceAgent
```

常用运维命令：

```powershell
Restart-Service ZeroTraceAgent
Stop-Service ZeroTraceAgent
.\zerotrace-agent-service.exe uninstall
```

修改本地 YAML 后需要重启服务：

```powershell
Restart-Service ZeroTraceAgent
```

## 7. 从源码编译

Windows x64 版本当前推荐在 Ubuntu/Debian Linux 环境中使用 Rust Windows GNU
目标交叉编译，编译时需要：

- MinGW-w64 x86_64 工具链；
- Rust `x86_64-pc-windows-gnu` target；
- Npcap SDK（编译期链接库）；
- 项目依赖和 Cargo 工具链。

完整命令见 [`docs/user/build.md`](./build.md) 的“编译 Windows x64 版本”一节。

注意区分：

- **Npcap SDK**：只用于编译，提供 `wpcap.lib` 等导入库；
- **Npcap**：必须安装在运行 Agent 的 Windows 主机上，提供驱动、`wpcap.dll` 和
  `Packet.dll`。

编译产物通常为：

```text
target/x86_64-pc-windows-gnu/release/zerotrace-agent.exe
target/x86_64-pc-windows-gnu/release/zerotrace-agent-ctl.exe
```

编译完成后，将两个 `.exe` 和 `config/zerotrace-agent-windows.yaml` 一起复制到
Windows 安装目录，并按照本文前面的步骤安装 Npcap、修改 YAML 和启动 Agent。

## 8. 常见问题

### 8.1 `wpcap.dll` 或 `Packet.dll` 找不到

检查：

```powershell
Get-Service npcap
Get-ChildItem "$env:WINDIR\System32\Npcap" -ErrorAction SilentlyContinue
```

如果服务不存在或 DLL 缺失，重新安装 x64 Npcap，并启用 WinPcap 兼容模式。确认运行
的是 x64 Agent，不要混用 32 位 DLL。

### 8.2 日志提示没有匹配的采集网卡

先查看实际网卡名称：

```powershell
Get-NetAdapter | Format-Table ifIndex, Name, Status, MacAddress -AutoSize
```

再修改 Agent Group 的 `inputs.cbpf.af_packet.interface_regex`。正则匹配的是友好
名称，不是 `\Device\NPF_{GUID}`；Agent 会在内部把友好名称转换为 Npcap 设备名。

### 8.3 Agent 能注册但没有流量

依次检查：

1. 选中的网卡是否真的承载目标流量；
2. `Test-NetConnection` 是否能访问 Controller 和 Ingester；
3. Agent Group 配置是否已经同步；
4. Windows 防火墙或终端安全软件是否拦截 Npcap；
5. 目标流量是否实际发生在 Windows 主机，而不是 WSL2/Docker Desktop Linux VM
   内部；
6. 是否错误地把网卡正则配置成了 `br-*` 或 `veth*`。

### 8.4 为什么抓不到 Docker Desktop 容器之间的流量

Docker Desktop 的 Linux 容器通过 Linux VM 内的 `veth` 和 `br-*` 通信。Windows Agent
只能通过 Npcap 打开 Windows 网卡，不能打开 Linux VM 内的接口。因此：

```text
frontend 容器 -> Linux veth -> Linux br-* -> Linux veth -> api 容器
```

这条路径不经过 Windows NDIS 网卡，Windows Npcap Agent 无法看到。只有经过 Windows/VM
边界的流量，才可能在 `vEthernet` 等 Windows 接口上出现。需要采集 Linux VM 内部
流量时，应在对应 Linux 网络命名空间中运行 Linux 版本 Agent。

### 8.5 Agent 启动后立即退出

先以前台方式执行并保留完整输出：

```powershell
Set-Location C:\ZeroTrace\Agent
$env:RUST_LOG = 'debug'
.\zerotrace-agent.exe -f .\zerotrace-agent.yaml
```

重点检查：

- YAML 路径和格式；
- `controller-ips` 是否填写；
- Npcap 服务是否运行；
- 日志目录是否可写；
- Agent 与 `zerotrace-agent-ctl.exe` 是否来自同一构建版本。

### 8.6 如何停止 Agent

前台运行时按 `Ctrl+C`。服务模式使用：

```powershell
Stop-Service ZeroTraceAgent
```

如需强制停止：

```powershell
Stop-Process -Name zerotrace-agent -Force
```

强制停止只建议用于 Agent 无法正常响应的情况。
