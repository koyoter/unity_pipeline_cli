# Unity Pipeline CLI

[中文](README.md) | [English](README.en.md)

**让 AI Agent 直接操控正在运行的 Unity Editor。** `unity_pipeline_cli` 把 `com.unity.pipeline` 包在 Editor 内提供的本地 HTTP 服务桥接成 MCP 工具，Claude Desktop 等 AI 客户端即插即用；同时提供命令行手动控制、Editor 实例发现与带补丁的一键安装。

本工程是官方 [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) 中 **Pipeline 包交互环节**的精简优化版：官方 CLI 负责安装编辑器、创建项目，而控制 Editor 本身需要单独的 `com.unity.pipeline` 包——本工具把这一环做成一个零重依赖的 Rust 单二进制（Windows 10+ / macOS，Intel 与 Apple Silicon）。

**以支持官方 Unity CLI 之外的 Unity 版本为目标**：上游 `com.unity.pipeline` 包在旧版 Editor 上开箱不可用，本工具以内置补丁打通，目前已稳定支持 **Unity 2022.3**。

## 核心能力

- 🤖 **AI 桥接（MCP，主打）**：Editor 内注册的每条管线指令自动暴露为 MCP 工具（含 JSON Schema 参数描述）。连接失效自动重连、Editor 忙碌自动重试、截图直接以图片回传、Bearer Token 全程脱敏。
- 🛠 **一键安装**：从 UPM registry 拉取 `com.unity.pipeline`，SHA1 校验，按包版本自动匹配内置补丁后装入运行中的项目，解决上游包在 Unity 2022 等环境开箱不可用的问题。
- 🎮 **命令直控**：`command` 列出或执行 Editor 内的全部管线指令，命令名与 JSON 参数原样转发，`--json` 机器可读输出，适合脚本与 CI。
- 🔍 **实例发现**：`projects` 列出所有运行中的 Editor 及其 Pipeline 服务状态（端口 / 版本 / 可达性）。

## 超出官方 CLI 的增强

官方 `unity` CLI 覆盖编辑器安装、项目创建与 Pipeline 包的基础交互；本工具在同一个环节上做了官方没有（或只停留在文档约定）的能力：

| 增强点 | 官方 CLI | 本工具 |
| :--- | :--- | :--- |
| 🤖 **MCP 桥（主打）** | ❌ 无 MCP 形态 | Editor 与 Player 的指令目录自动暴露为 MCP 工具——纯转发零硬编码，包 0.6 → 0.9 全兼容，AI 客户端即插即用 |
| 📱 **Android Mono 动态执行** | ❌ 上游裁掉 Android 的 Roslyn 路径，`script.eval` 报 "Platform Not Supported" | ✅ 内置补丁打通 |
| 🧩 **旧版 Editor 兼容** | ❌ 开箱仅支持部分版本 | ✅ 内置补丁稳定支持 Unity 2022.3 |
| 🎮 **Player 调试全托管** | 📄 `--runtime`/`--runtime-path` 仅文档约定 | ✅ 官方同款 flag 全实现，另补 `--adb <包名>`：run-as 读描述符 + adb forward + 断线/应用重启自愈，`command` 与 MCP 共用 |
| 🛟 **调试韧性** | ❌ 连接断即失效 | ✅ Player 崩溃后 MCP 连接仍可用（工具表缓存 + `player_status`/`player_log_read` 本地工具）；Player 晚于连接启动时自动推送 `tools/list_changed` |

## 快速上手

### 前提

- Windows 10+ 或 macOS（安装时会用到系统自带的 `curl` / `tar`）
- 目标项目已在 Unity Editor 中打开（所有与 Editor 交互的命令都要求实例正在运行）

### 获取

从 [GitHub Releases](https://github.com/koyoter/unity_pipeline_cli/releases/latest) 下载对应平台的压缩包（文件名含平台标识，拿不准 Mac 架构就选 universal），解压即用——单文件、免安装。

已装了旧版本？直接 `unity upgrade` 自更新，无需手动下载（详见[命令参考](#命令参考)）。

自行编译：`cargo build --release`，或用根目录脚本（`build-release.bat` / `./build-release.sh universal`）。

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

### `upgrade` — 自更新

自动检测 GitHub 上的新版本：有更新时展示更新内容，确认后自动下载（带进度条）并替换当前程序，无需手动下载安装。

### `language` — 查看 / 切换界面语言

```bash
unity_pipeline_cli language      # 查看当前语言
unity_pipeline_cli language zh   # 切换到中文（en / zh）
```

支持英文与中文。不设置时按以下顺序自动判定：本命令保存的设置 → `LANGUAGE` / `LC_ALL` / `LANG` 环境变量 → 操作系统界面语言（Windows API）→ 英文。设置保存在平台配置目录（Windows 为 `%APPDATA%\unity_pipeline_cli\language.txt`）。

### `command` — 列出 / 执行管线指令

```bash
unity_pipeline_cli command                                  # 列出全部指令
unity_pipeline_cli command editor.status                    # 执行无参指令（点号形式）
unity_pipeline_cli command "asset find" '{"type":"Material"}'  # 带参执行（空格全名 + JSON 参数）
```

命令名原样转发，服务端自己解析：分组命名（包 0.9+）可写 `"asset find"` 或 `asset.find`，老版本包用扁平名（如 `run_tests`）。参数是一个 JSON 对象字符串，同样原样转发，校验由服务端完成（错误连 `argProblems` 明细一起透传）。

| 参数 | 说明 |
| :--- | :--- |
| `<name>` | 指令名，原样转发；省略则列出全部可用指令 |
| `[params]` | 指令参数，JSON 对象字符串 |
| `--project-path <路径>` | 多个 Editor 同时运行时锁定目标 |
| `--runtime <进程名>` / `--runtime-path <目录>` / `--adb <包名>` / `--adb-serial <serial>` | 目标改为运行中的 Player（见[调试打包后的程序](#调试打包后的程序-player)） |
| `--timeout <秒>` | 同时限定本地等待与服务端执行预算（默认 30） |
| `--json` | 机器可读输出 |

### `mcp` — MCP 服务端（由 AI 客户端拉起，勿手动运行）

在 stdio 上运行 JSON-RPC，把管线指令目录暴露为 MCP 工具。`--project-path <路径>` 可绑定具体项目——多个 Editor 并存时必须分别绑定，否则会因歧义启动失败；`--runtime` / `--runtime-path` / `--adb` 则指向运行中的 Player（见[调试打包后的程序](#调试打包后的程序-player)）。

特性：

- 指令目录自动映射为 tools；Editor 域重载后新增的指令会通过 `tools/list_changed` 通知客户端刷新
- 连接失效（Editor 重启 / 域重载）自动重新发现；Editor 忙碌时按服务端 `Retry-After` 自动重试（至多 30 次）；被模态对话框阻塞时直接报错并转发 Editor 给出的提示
- 任何返回 PNG 的指令（截图类）自动以 MCP 图片内容返回（base64）；任何自带 `timeout` / `timeout_s`（秒）参数的指令，该超时端到端生效
- 工具名与命令目录同源（`asset.create` 一类点号全名），跨组唯一；MCP 层零硬编码命令名，包 0.6 → 0.9 全兼容
- 所有返回文本中的 Bearer Token 一律替换为 `[redacted]`
- 兼容 MCP 协议版本 `2024-11-05` / `2025-03-26` / `2025-06-18`

### `configure mcp` — 生成客户端配置片段

```bash
unity_pipeline_cli configure mcp                            # 一次打印全部三节（见下）
unity_pipeline_cli configure mcp --project-path "D:\proj"   # 只打编辑器节（绑定项目）
unity_pipeline_cli configure mcp --runtime-path "C:\Builds\MyGame"   # 只打桌面 Player 节
unity_pipeline_cli configure mcp --adb com.acme.mygame      # 只打 Android Player 节
```

不带 flag 一次打印三节——**编辑器 / 桌面 Player / Android Player**——每节都是可直接粘贴的独立 `mcpServers` 条目（`--trace` 给条目附上 `UNITY_MCP_TRACE` 协议日志）。三节的具体形态与差异见[调试打包后的程序](#调试打包后的程序-player)。

## 调试打包后的程序（Player）

`--runtime`（按进程/可执行名，官方 CLI 同款）、`--runtime-path`（按 `.unity-pipeline-runtime-port` 所在位置：Windows 传构建目录、macOS 传 `.app` 包路径）指向本地桌面 Player；`--adb <包名>`（多设备加 `--adb-serial <serial>`）指向 Android 上的 Player——CLI 全托管：run-as 读设备上的描述符（port + token）→ `adb forward` 转发到 127.0.0.1，断线 / 应用重启后自动重建。`command` 与 `mcp` 共用这套目标选择。

**前提**：Player 必须是**开发构建**且启用了 runtime server（`enableInDevelopmentBuilds`）。serverInfo 名字带目标标识（如 `unity-mcp@MyGame`），agent 侧多个连接不会混。

### 指令支持范围：脚本后端决定

Player 端 runtime server 暴露 18 条 runtime 指令，但**能力按打包时的脚本后端与平台分档**——要全部指令，打包选 **Mono**：

| 能力 | 桌面 Mono dev 构建 | 桌面 IL2CPP dev 构建 | Android（Mono dev） | Android / iOS（IL2CPP） |
| :--- | :--- | :--- | :--- | :--- |
| runtime 服务器与其余指令（`console.get`、`runtime status`、input、capture、`script.methodreload`…） | ✅ | ✅ | ✅ | ✅ |
| `script.eval` / `script.eval_file`（动态编译执行） | ✅ | ❌ 编译过但 `Assembly.Load` 在 IL2CPP 下不可用 | ✅ 本工具补丁打通（上游裁剪了 Android 的 Roslyn 路径与引用收集，补丁改走 APK 内读 DLL + AssetManager 流式引用；首次调用需 Roslyn 预热，建议 `--timeout 120` 起） | ❌ `Assembly.Load` 不可用（上游对非桌面平台根本不编入 Roslyn 路径，报 "Platform Not Supported"） |
| `script.methodreload` 热更 | ✅ | ✅（经 IL 解释器执行：受限 C# 子集 + 固定 host 面） | ✅（同左） | ✅（同左） |

调动态逻辑优先走 `script.methodreload`（全后端可用）；`script.eval` 在桌面 Mono 与 Android Mono dev 构建可用（Android 依赖本工具补丁）。

### Android 排障速查

真机联调最常见的两个构建侧坑（现象 → 判据 → 修复）：

| 问题 | 现象 | 判据 | 修复 |
| :--- | :--- | :--- | :--- |
| runtime pipeline 被整个裁掉 | `run-as cat` 报 "No such file or directory"，但 app 明明在跑 | `files/` 目录全空 + logcat 零条 "Pipeline:" 行（健康应用启动即打 "Runtime server started successfully on port N"） | 编辑器里 `settings.runtime_pipeline set` 打开 `enableInDevelopmentBuilds` → recompile → 重打包 |
| 包不可调试 | 重打包后 `run-as` 报 "package not debuggable" | — | 重打包显式带 `build.run` 的 `options: ["Development", "AllowDebugging"]`——只靠 EditorUserBuildSettings 的 developmentBuild 不可靠 |

另注意：Android 的 `persistentDataPath` 落点设备相关（内部 `/data/data/<pkg>/files` 或外部 `/sdcard/Android/data/<pkg>/files`），CLI 已做三级候选自动兼容（内部 → 外部 → 解析 logcat 里 publisher 自报的写入路径），无需手动干预。

### MCP 三条目与降级行为

Player 是一条**独立连接**，与 Editor 条目并存（`configure mcp` 可直接生成）：

```json
{
  "mcpServers": {
    "unity_pipeline_cli-player": {
      "command": "C:\\path\\to\\unity_pipeline_cli.exe",
      "args": ["mcp", "--runtime-path", "C:\\Builds\\MyGame"]
    },
    "unity_pipeline_cli-android": {
      "command": "C:\\path\\to\\unity_pipeline_cli.exe",
      "args": ["mcp", "--adb", "com.acme.mygame"]
    }
  }
}
```

- 工具表来自该 Player 的 runtime server，直接以 MCP 工具调用，无需另起 CLI；Player 在 MCP 连接**之后**启动时，目录监视线程自动探到并推送 `tools/list_changed`，宿主无需手动刷新
- 降级可用：成功拉到工具表后缓存在构建目录（Android 在本地临时目录），Player 崩溃后重启 MCP 仍能列出工具集，调用则明确报离线错误
- 两个本地工具始终可用：`player_status`（查 Player 是否在跑，不在跑时给出启动提示）、`player_log_read`（桌面读 Player.log，Android 读 `adb logcat -d -s Unity`）

## 附录

### 补丁机制

- 内置补丁编译期内嵌进 exe（`build.rs`），扁平存放于 `patches/`：一个上游目标文件一个补丁文件，命名 `<目标路径编码>@<自增长序号>.patch`；版本适配规则在 `patches/manifest.json`（每个补丁集声明 `from` 起始版本和整表补丁清单，取 `from ≤ 包版本` 的最大者整表应用，如 `0.9.0` 用 `0.9` 档、比最后一档还新的版本沿用该档）；安装时先 dry-run，通过后才应用
- 自定义补丁放在运行目录 `patchs/custom/`，命名 `modify_unity_local_1.patch`、`modify_unity_local_2.patch`……按编号升序在内置补丁之后应用
- `patchs/system/` 是 exe 内置补丁中当前选中补丁集的自动镜像，每次安装重建，请勿手改

### 构建发布

```bash
cargo build --release              # 通用
build-release.bat                  # Windows 一键产出 releases/
./build-release.sh universal       # macOS：intel / arm / universal / all
```

### 与官方 Unity CLI 的关系

官方 [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) 负责编辑器安装、项目创建、版本控制接入等；控制 Editor 本身需另装 `com.unity.pipeline` 包。本工程只做并增强这一环（带补丁的一键安装、实例发现、指令执行、MCP 桥接）；不包含编辑器安装、项目创建、Unity 账号 / Cloud 集成。相对官方的具体增强清单见[超出官方 CLI 的增强](#超出官方-cli-的增强)。版本覆盖以官方 CLI 之外为目标：内置补丁让上游包跑在它开箱不支持的 Editor 上，目前已稳定支持 **Unity 2022.3**。
