# Unity Pipeline CLI

[中文](README.md) | [English](README.en.md)

**让 AI Agent 直接操控正在运行的 Unity Editor。** `unity_pipeline_cli` 把 `com.unity.pipeline` 包在 Editor 内提供的本地 HTTP 服务桥接成 MCP 工具，Claude Desktop 等 AI 客户端即插即用；同时提供命令行手动控制、Editor 实例发现与带补丁的一键安装。

本工程是官方 [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) 中 **Pipeline 包交互环节**的精简优化版：官方 CLI 负责安装编辑器、创建项目，而控制 Editor 本身需要单独的 `com.unity.pipeline` 包——本工具把这一环做成一个零重依赖的 Rust 单二进制（Windows 10+ / macOS，Intel 与 Apple Silicon）。

## 核心能力

- 🤖 **AI 桥接（MCP，主打）**：Editor 内注册的每条管线指令自动暴露为 MCP 工具（含 JSON Schema 参数描述）。连接失效自动重连、Editor 忙碌自动重试、`recompile` 等到编译结束才返回、截图直接以图片回传、Bearer Token 全程脱敏。
- 🛠 **一键安装**：从 UPM registry 拉取 `com.unity.pipeline`，SHA1 校验，按包版本自动匹配内置补丁后装入运行中的项目，解决上游包在 Unity 2022 等环境开箱不可用的问题。
- 🎮 **命令直控**：`command` 列出或执行 Editor 内的全部管线指令，参数本地校验，`--json` 机器可读输出，适合脚本与 CI。
- 🔍 **实例发现**：`projects` 列出所有运行中的 Editor 及其 Pipeline 服务状态（端口 / 版本 / 可达性）。

## 快速上手

### 前提

- Windows 10+ 或 macOS（安装时会用到系统自带的 `curl` / `tar`）
- 目标项目已在 Unity Editor 中打开（所有与 Editor 交互的命令都要求实例正在运行）

### 获取

- Windows：直接使用仓库 `releases/unity_pipeline_cli.exe`
- macOS / 自行编译：`cargo build --release`，或用根目录脚本（`build-release.bat` / `./build-release.sh universal`）

### 一条命令接入

```bash
unity_pipeline_cli install
```

选好运行中的 Editor 与包版本，工具会自动完成下载、打补丁、装入项目，并在结束时打印已绑定该项目的 MCP 配置片段——把它粘贴到 Claude Desktop / Antigravity 等客户端，重启客户端，模型即可列出并调用 Editor 内的全部管线指令。

想先手动看看状态或指令？`unity_pipeline_cli projects`、`unity_pipeline_cli command`。

## 命令参考

### `install` — 安装 `com.unity.pipeline`

交互式选择运行中的 Editor 与包版本，随后自动完成：下载（系统 `curl`）→ SHA1 校验 → 解压（系统 `tar`）→ 应用内置补丁 → 装入 `<项目>/Packages/com.unity.pipeline` → 打印已绑定该项目的 MCP 配置片段。回到 Editor 等待自动导入即可。

| 参数 | 说明 |
| :--- | :--- |
| `--version <版本>` | 跳过版本选择，安装指定版本 |
| `--latest` | 直接安装 registry 的 `latest` |
| `--keep-cache` | 复用 `downloads/` 中已下载的 tgz，不重新下载 |

### `projects` — 查看运行中的 Editor

列出项目名、PID、Unity 版本、Pipeline 包版本、服务端口与可达性。`--json` 输出机器可读 JSON。

### `command` — 列出 / 执行管线指令

```bash
unity_pipeline_cli command                    # 列出全部指令及参数说明
unity_pipeline_cli command editor_play        # 执行无参指令
unity_pipeline_cli command eval --code "1+1"  # 带参执行
```

| 参数 | 说明 |
| :--- | :--- |
| `<name>` | 指令名；省略则列出全部可用指令 |
| `--<param> <value>` | 指令参数，支持 `--k=v`、裸 `--flag`（按 `true` 处理）、位置参数（按声明顺序映射到必选参数） |
| `--project-path <路径>` | 多个 Editor 同时运行时锁定目标 |
| `--timeout <秒>` | 同时限定本地等待与服务端执行预算（默认 30） |
| `--json` | 机器可读输出 |

未知参数会在本地直接拦截，并打印该指令的参数说明。

### `mcp` — MCP 服务端（由 AI 客户端拉起，勿手动运行）

在 stdio 上运行 JSON-RPC，把管线指令目录暴露为 MCP 工具。`--project-path <路径>` 可绑定具体项目——多个 Editor 并存时必须分别绑定，否则会因歧义启动失败。

特性：

- 指令目录自动映射为 tools；Editor 域重载后新增的指令会通过 `tools/list_changed` 通知客户端刷新
- 连接失效（Editor 重启 / 域重载）自动重新发现；Editor 忙碌时按服务端 `Retry-After` 自动重试（至多 30 次）；被模态对话框阻塞时直接报错并转发 Editor 给出的提示
- `recompile` 自动轮询状态直到编译结束；`run_tests` / `wait_for` 自带的超时参数端到端生效
- `capture_game_view` / `capture_scene_view` 以 MCP 图片内容返回（base64 PNG）
- 所有返回文本中的 Bearer Token 一律替换为 `[redacted]`
- 兼容 MCP 协议版本 `2024-11-05` / `2025-03-26` / `2025-06-18`

### `configure mcp` — 生成客户端配置片段

```json
{
  "mcpServers": {
    "unity_pipeline_cli": {
      "command": "C:\\path\\to\\unity_pipeline_cli.exe",
      "args": ["mcp"]
    }
  }
}
```

未绑定项目时：恰好 1 个可用 Editor 会自动连接；并存多个时会因歧义失败，需在 `args` 中追加 `"--project-path" "<项目绝对路径>"`（每个项目一条 `mcpServers` 条目，互不干扰）。

## 附录

### 补丁机制

- 内置补丁编译期内嵌进 exe（`build.rs`），按包版本分集：`< 0.7.0-exp.1` → `legacy`，`≥ 0.7.0-exp.1` → `v0.7`；安装时先 dry-run，通过后才应用
- 自定义补丁放在运行目录 `patchs/custom/`，命名 `modify_unity_local_1.patch`、`modify_unity_local_2.patch`……按编号升序在内置补丁之后应用
- `patchs/system/` 是 exe 内置补丁的自动镜像，每次安装重建，请勿手改

### 构建发布

```bash
cargo build --release              # 通用
build-release.bat                  # Windows 一键产出 releases/
./build-release.sh universal       # macOS：intel / arm / universal / all
```

### 与官方 Unity CLI 的关系

官方 [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) 负责编辑器安装、项目创建、版本控制接入等；控制 Editor 本身需另装 `com.unity.pipeline` 包。本工程只做并增强这一环（带补丁的一键安装、实例发现、指令执行、MCP 桥接）；不包含编辑器安装、项目创建、Unity 账号 / Cloud 集成。
