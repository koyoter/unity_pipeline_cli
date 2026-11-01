# Unity Pipeline CLI

[中文](README.md) | [English](README.en.md)

**Let AI agents drive a running Unity Editor directly.** `unity_pipeline_cli` bridges the local HTTP service exposed by the `com.unity.pipeline` package inside the Editor into MCP tools, ready to plug into AI clients such as Claude Desktop. It also provides manual command-line control, Editor instance discovery, and one-shot patched installation.

A single self-contained Rust binary (Windows 10+ / macOS, Intel and Apple Silicon). The official [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) installs Editors and creates projects; controlling the Editor itself requires the separate `com.unity.pipeline` package — that slice is what this tool does and enhances, with built-in patches bridging upstream packages that do not work out of the box on older Editors. **Unity 2022.3** is already stably supported (full positioning in [Relation to the official Unity CLI](#relation-to-the-official-unity-cli)).

## Core capabilities

- 🤖 **AI bridging (MCP, the headline)**: every pipeline command registered inside the Editor is automatically exposed as an MCP tool (with a JSON Schema description of its parameters). Dropped connections are re-established automatically, busy Editors are retried, screenshots come back as images, and Bearer tokens are redacted everywhere.
- 🛠 **One-shot install**: fetches `com.unity.pipeline` from the UPM registry, verifies SHA1, matches built-in patches against the package version, and installs into a running project — fixing upstream packages that do not work out of the box on environments like Unity 2022.
- 🎮 **Direct command control**: `command` lists or executes every pipeline command inside the Editor, forwarding names and JSON parameters verbatim, with `--json` machine-readable output for scripts and CI.
- 🔍 **Instance discovery**: `projects` lists every running Editor and its Pipeline service status (port / version / reachability).

## What goes beyond the official CLI

This tool adds capabilities the official CLI does not have (or only documents as conventions) on top of the same Pipeline-interaction slice:

| Enhancement | Official CLI | This tool |
| :--- | :--- | :--- |
| 🤖 **MCP bridge (headline)** | ❌ no MCP form | Editor and Player command catalogs are automatically exposed as MCP tools — pure forwarding with zero hard-coded names, compatible with package 0.6 → 0.9, plug-and-play for AI clients |
| 📱 **Android Mono dynamic execution** | ❌ upstream compiles the Roslyn path out of Android, `script.eval` reports "Platform Not Supported" | ✅ enabled by a built-in patch |
| 🧩 **Older Editor compatibility** | ❌ supports only some versions out of the box | ✅ built-in patches stably support Unity 2022.3 |
| 🎮 **Fully managed Player debugging** | ✅ desktop: `--runtime`/`--runtime-path`; on Android you must handle adb forwarding and fetch the debug port/token yourself | ✅ those same flags plus `--adb <package>`: descriptor via run-as (port + token) → adb forward → self-healing after disconnects/app restarts, **zero manual steps**, shared by `command` and MCP |
| 🛟 **Debugging resilience** | ❌ connection lost = tools gone | ✅ the MCP connection stays usable after a Player crash (tool-catalog cache + `player_status`/`player_log_read` local tools); `tools/list_changed` is pushed automatically when the Player starts after the connection |

## Quick start

### Prerequisites

- Windows 10+ or macOS (installation uses the built-in `curl` / `tar`)
- The target project is open in a running Unity Editor (every command that talks to the Editor requires a live instance)

### Getting the binary

Download the archive for your platform from [GitHub Releases](https://github.com/koyoter/unity_pipeline_cli/releases/latest) (filenames carry the platform tag; if unsure about your Mac's architecture, pick universal) and extract it — a single self-contained binary, no installation. You can also build it yourself (see [Build & release](#build--release)).

Already have an older build? Run `unity_pipeline_cli upgrade` to self-update — no manual download needed (see the [command reference](#command-reference)).

### One command to wire it up

```bash
unity_pipeline_cli install
```

Pick a running Editor and a package version; everything else is automatic — download, patching, installation — and the tool prints an MCP config snippet bound to that project. Paste it into Claude Desktop / Antigravity etc., restart the client, and the model can list and invoke every pipeline command inside the Editor.

Want a manual look first? `unity_pipeline_cli projects`, `unity_pipeline_cli command`.

## Command reference

### `install` — install `com.unity.pipeline`

Interactively pick a running Editor and a package version, then it automatically: downloads (system `curl`) → verifies SHA1 → extracts (system `tar`) → applies built-in patches → installs into `<project>/Packages/com.unity.pipeline` → prints an MCP config snippet bound to that project. Switch back to the Editor and wait for the automatic import.

| Flag | Description |
| :--- | :--- |
| `--version <version>` | Skip the version picker and install this exact version |
| `--latest` | Install the registry's `latest` directly |
| `--keep-cache` | Reuse the tgz already in `downloads/` instead of re-downloading |

### `mcp` — MCP server (launched by the AI client; do not run by hand)

Speaks JSON-RPC over stdio and exposes the pipeline command catalog as MCP tools. `--project-path <path>` pins a specific project — with multiple Editors running, each entry must be pinned, otherwise startup fails due to ambiguity; `--runtime` / `--runtime-path` / `--adb` target a running Player instead (see [Debugging a built program](#debugging-a-built-program-player)).

Features:

- The command catalog maps automatically onto tools; commands that appear after an Editor domain reload are pushed to the client via `tools/list_changed`
- Stale connections (Editor restart / domain reload) are re-discovered automatically; a busy Editor is retried honoring the server's `Retry-After` (up to 30 attempts); a modal dialog blocking the Editor fails fast and forwards the guidance the Editor published
- Any command returning a PNG (the capture family) automatically comes back as MCP image content; any command carrying a `timeout` / `timeout_s` (seconds) argument has it honored end to end
- Tool names derive from the server catalog (`asset.create`-style dotted full names), unique across groups; the MCP layer hard-codes no command names and works with package 0.6 → 0.9
- Bearer tokens in any returned text are replaced with `[redacted]`
- Speaks MCP protocol versions `2024-11-05` / `2025-03-26` / `2025-06-18`

### `command` — list / execute pipeline commands

```bash
unity_pipeline_cli command                                     # list all commands
unity_pipeline_cli command editor.status                       # run a parameterless command (dotted form)
unity_pipeline_cli command "asset find" '{"type":"Material"}'  # run with arguments (space name + JSON params)
```

The command name is forwarded verbatim and resolved by the server: grouped naming (package 0.9+) accepts both `"asset find"` and `asset.find`, pre-0.9 packages use flat names (e.g. `run_tests`). Parameters are one JSON object string, also forwarded verbatim — validation happens server-side and errors (including `argProblems` details) pass through.

| Argument | Description |
| :--- | :--- |
| `<name>` | Command name, forwarded verbatim; omit to list all available commands |
| `[params]` | Command parameters as a JSON object string |
| `--project-path <path>` | Pin the target when multiple Editors are running |
| `--runtime <name>` / `--runtime-path <dir>` / `--adb <package>` / `--adb-serial <serial>` | Target a running Player instead (see [Debugging a built program](#debugging-a-built-program-player)) |
| `--timeout <seconds>` | Bounds both the local wait and the server-side execution budget (default 30) |
| `--json` | Machine-readable output |

### `configure mcp` — print a client config snippet

```bash
unity_pipeline_cli configure mcp                            # print all three sections at once (see below)
unity_pipeline_cli configure mcp --project-path "D:\proj"   # Editor section only (pinned)
unity_pipeline_cli configure mcp --runtime-path "C:\Builds\MyGame"   # desktop Player section only
unity_pipeline_cli configure mcp --adb com.acme.mygame      # Android Player section only
```

Bare `configure mcp` prints all three sections at once — **Editor / desktop Player / Android Player** — each a standalone paste-ready `mcpServers` entry (`--trace` attaches a `UNITY_MCP_TRACE` protocol log to the entries). See [Debugging a built program](#debugging-a-built-program-player) for what each form does.

### `projects` — inspect running Editors

Lists project name, PID, Unity version, installed Pipeline package version, server port and reachability. `--json` emits machine-readable JSON.

### `upgrade` — self-update

Automatically checks GitHub for a new version: when one exists, it shows the release notes and, after your confirmation, downloads (with a progress bar) and replaces the running binary — no manual download needed.

### `language` — show / switch UI language

```bash
unity_pipeline_cli language      # show the current language
unity_pipeline_cli language zh   # switch to Chinese (en / zh)
```

English and Chinese are supported. Without an explicit setting the language is auto-detected in this order: the setting saved by this command → `LANGUAGE` / `LC_ALL` / `LANG` environment variables → OS UI language (Windows API) → English. The setting is stored in the platform config directory (`%APPDATA%\unity_pipeline_cli\language.txt` on Windows).

## Debugging a built program (Player)

**Prerequisite: to use pipeline capabilities in a built program, enable it in the Editor FIRST, then build** — whether the runtime pipeline is compiled in is decided at **build time**; enabling it afterwards has no effect on an already-built package. To enable: in the Editor, Project Settings → Pipeline → Runtime → check `enableInDevelopmentBuilds` (or enable it via this tool's `settings.runtime_pipeline set` → recompile), then rebuild (a development build for debugging). For Android on-device debugging — development-build options, launch self-check, and the common pitfalls — see [Android troubleshooting quick reference](#android-troubleshooting-quick-reference).

Which program is targeted is decided by the target selection shared by `command` and `mcp`: with no flag, the Editor; `--runtime` (by process/executable name, matching the official CLI), `--runtime-path` (by where `.unity-pipeline-runtime-port` lives: Windows — the build dir; macOS — the `.app` bundle) target a local desktop Player; `--adb <package>` (plus `--adb-serial <serial>` with several devices) targets the Player on Android — fully managed by the CLI: the descriptor on the device is read via run-as (port + token), then an `adb forward` exposes it at 127.0.0.1, re-established automatically after disconnects / app restarts.

serverInfo carries the target label (e.g. `unity-mcp@MyGame`), so multiple agent-side connections never blur together.

### Command coverage depends on the scripting backend

The player's runtime server exposes 18 runtime commands, but **capability tiers follow the scripting backend and platform chosen at build time** — choose **Mono** when packing to get all of them:

| Capability | Desktop Mono dev build | Desktop IL2CPP dev build | Android (Mono dev) | Android / iOS (IL2CPP) |
| :--- | :--- | :--- | :--- | :--- |
| Runtime server and the remaining commands (`console.get`, `runtime status`, input, capture, `script.methodreload`, …) | ✅ | ✅ | ✅ | ✅ |
| `script.eval` / `script.eval_file` (dynamic compile & run) | ✅ | ❌ compiles, but `Assembly.Load` is unavailable under IL2CPP | ✅ enabled by this tool's patch (upstream compiles the Roslyn path and its reference lookup out on Android; the patch reads the DLLs from the APK via AssetManager streams instead — first call JIT-warms Roslyn, start with `--timeout 120`) | ❌ `Assembly.Load` unavailable (upstream compiles the Roslyn path out of non-desktop platforms entirely — "Platform Not Supported") |
| `script.methodreload` hot reload | ✅ | ✅ (runs through the IL interpreter: restricted C# subset + fixed host surface) | ✅ (same) | ✅ (same) |

Prefer `script.methodreload` for dynamic logic (works on every backend); `script.eval` is usable on desktop Mono and — via this tool's patch — Android Mono dev builds.

### Android troubleshooting quick reference

- Rebuild as a development build (with this tool's `build.run`, pass `options: ["Development", "AllowDebugging"]` — otherwise the package is not debuggable)
- Self-check: after launching the app, `adb logcat -d -s Unity` should show "Runtime server started successfully on port N"; if not, the runtime pipeline never made it in

The two most common build-side pitfalls when testing on a device (symptom → evidence → fix):

| Problem | Symptom | Evidence | Fix |
| :--- | :--- | :--- | :--- |
| Runtime pipeline stripped entirely | `run-as cat` says "No such file or directory" while the app is clearly running | `files/` directory empty + zero "Pipeline:" lines in logcat (a healthy app logs "Runtime server started successfully on port N" at launch) | In the Editor run `settings.runtime_pipeline set` to enable `enableInDevelopmentBuilds` → recompile → rebuild |
| Package not debuggable | after a rebuild `run-as` says "package not debuggable" | — | Rebuild with `build.run`'s explicit `options: ["Development", "AllowDebugging"]` — relying on EditorUserBuildSettings' developmentBuild alone is unreliable |

Also note: on Android the `persistentDataPath` location is device-dependent (internal `/data/data/<pkg>/files` or external `/sdcard/Android/data/<pkg>/files`); the CLI handles this with a three-candidate fallback (internal → external → the publisher's logcat-reported write path), no manual step needed.

### The three MCP entries and degraded behavior

The Player is its **own connection**, alongside the Editor entry (`configure mcp` generates these):

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

- The tool list comes from that Player's runtime server and is invoked directly as MCP tools — no separate CLI call; when the Player starts AFTER the MCP connection, the catalog watcher thread detects it and pushes `tools/list_changed`, so the host re-lists without manual refresh
- Degraded mode: the fetched catalog is cached in the build directory (a local temp dir on Android), so after a crash a restarted MCP still lists the tool set while calls report a clear offline error
- Two local tools always work: `player_status` (reports whether the Player is running, with a hint on how to start it when not) and `player_log_read` (desktop: Player.log from disk; Android: the device's `adb logcat -d -s Unity`)

## Appendix

### Patch mechanism

- Built-in patches are embedded into the exe at compile time (`build.rs`), stored flat under `patches/`: one patch file per upstream target file, named `<encoded-target-path>@<auto-increment>.patch`; version adaptation rules live in `patches/manifest.json` (each patch set declares its `from` start version plus a full ordered patch list — the entry with the largest `from ≤ package version` applies as a whole, e.g. `0.9.0` uses the `0.9` set and anything newer than the last set reuses it); installation dry-runs them first and only applies on success
- Custom patches live in `patchs/custom/` under the working directory, named `modify_unity_local_1.patch`, `modify_unity_local_2.patch`, …, applied in ascending order after the built-ins
- `patchs/system/` is an auto-generated mirror of the selected embedded patch set, rebuilt on every install — do not edit

### Build & release

```bash
cargo build --release              # generic
build-release.bat                  # Windows: one-shot build into releases/
./build-release.sh universal       # macOS: intel / arm / universal / all
```

### Relation to the official Unity CLI

The official [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) covers Editor installation, project creation, version-control setup, and more; controlling the Editor itself requires the separate `com.unity.pipeline` package. This project does — and enhances — exactly that slice (patched one-shot install, instance discovery, command execution, MCP bridging); it does not install Editors, create projects, or integrate Unity accounts / Cloud. For the concrete list of enhancements over the official CLI, see [What goes beyond the official CLI](#what-goes-beyond-the-official-cli).
