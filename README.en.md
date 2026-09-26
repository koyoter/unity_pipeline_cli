# Unity Pipeline CLI

[中文](README.md) | [English](README.en.md)

**Let AI agents drive a running Unity Editor directly.** `unity_pipeline_cli` bridges the local HTTP service exposed by the `com.unity.pipeline` package inside the Editor into MCP tools, ready to plug into AI clients such as Claude Desktop. It also provides manual command-line control, Editor instance discovery, and one-shot patched installation.

This project is a streamlined, optimized take on the **Pipeline-package interaction** slice of the official [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli): the official CLI installs Editors and creates projects, while controlling the Editor itself requires the separate `com.unity.pipeline` package — this tool packs that slice into a single Rust binary with no heavyweight dependencies (Windows 10+ / macOS, Intel and Apple Silicon).

**The goal is to support Unity versions beyond the official Unity CLI**: the upstream `com.unity.pipeline` package does not work out of the box on older Editors; built-in patches bridge the gap, and **Unity 2022.3** is already stably supported.

## Core capabilities

- 🤖 **AI bridging (MCP, the headline)**: every pipeline command registered inside the Editor is automatically exposed as an MCP tool (with a JSON Schema description of its parameters). Dropped connections are re-established automatically, busy Editors are retried, `recompile` only returns once compilation finishes, screenshots come back as images, and Bearer tokens are redacted everywhere.
- 🛠 **One-shot install**: fetches `com.unity.pipeline` from the UPM registry, verifies SHA1, matches built-in patches against the package version, and installs into a running project — fixing upstream packages that do not work out of the box on environments like Unity 2022.
- 🎮 **Direct command control**: `command` lists or executes every pipeline command inside the Editor, validates parameters locally, and offers `--json` machine-readable output for scripts and CI.
- 🔍 **Instance discovery**: `projects` lists every running Editor and its Pipeline service status (port / version / reachability).

## Quick start

### Prerequisites

- Windows 10+ or macOS (installation uses the built-in `curl` / `tar`)
- The target project is open in a running Unity Editor (every command that talks to the Editor requires a live instance)

### Getting the binary

Download the archive for your platform from [GitHub Releases](https://github.com/koyoter/unity_pipeline_cli/releases/latest) (filenames carry the platform tag; if unsure about your Mac's architecture, pick universal) and extract it — a single self-contained binary, no installation.

Already have an older build? Run `unity upgrade` to self-update — no manual download needed (see the [command reference](#command-reference)).

Building yourself: `cargo build --release`, or the root scripts (`build-release.bat` / `./build-release.sh universal`).

### One command to wire it up

```bash
unity_pipeline_cli install
```

Pick a running Editor and a package version; the tool downloads, patches and installs the package automatically, then prints an MCP config snippet bound to that project — paste it into Claude Desktop / Antigravity etc., restart the client, and the model can list and invoke every pipeline command inside the Editor.

Want a manual look first? `unity_pipeline_cli projects`, `unity_pipeline_cli command`.

## Command reference

### `install` — install `com.unity.pipeline`

Interactively pick a running Editor and a package version, then it automatically: downloads (system `curl`) → verifies SHA1 → extracts (system `tar`) → applies built-in patches → installs into `<project>/Packages/com.unity.pipeline` → prints an MCP config snippet bound to that project. Switch back to the Editor and wait for the automatic import.

| Flag | Description |
| :--- | :--- |
| `--version <version>` | Skip the version picker and install this exact version |
| `--latest` | Install the registry's `latest` directly |
| `--keep-cache` | Reuse the tgz already in `downloads/` instead of re-downloading |

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

### `command` — list / execute pipeline commands

```bash
unity_pipeline_cli command                    # list all commands with their parameters
unity_pipeline_cli command editor_play        # run a parameterless command
unity_pipeline_cli command eval --code "1+1"  # run with arguments
```

| Argument | Description |
| :--- | :--- |
| `<name>` | Command name; omit to list all available commands |
| `--<param> <value>` | Command parameters; supports `--k=v`, bare `--flag` (treated as `true`), and positionals (mapped onto required parameters in declaration order) |
| `--project-path <path>` | Pin the target when multiple Editors are running |
| `--timeout <seconds>` | Bounds both the local wait and the server-side execution budget (default 30) |
| `--json` | Machine-readable output |

Unknown parameters are rejected locally, with the command's parameter reference printed.

### `mcp` — MCP server (launched by the AI client; do not run by hand)

Speaks JSON-RPC over stdio and exposes the pipeline command catalog as MCP tools. `--project-path <path>` pins a specific project — with multiple Editors running, each entry must be pinned, otherwise startup fails due to ambiguity.

Features:

- The command catalog maps automatically onto tools; commands that appear after an Editor domain reload are pushed to the client via `tools/list_changed`
- Stale connections (Editor restart / domain reload) are re-discovered automatically; a busy Editor is retried honoring the server's `Retry-After` (up to 30 attempts); a modal dialog blocking the Editor fails fast and forwards the guidance the Editor published
- `recompile` polls until compilation settles; the timeout arguments of `run_tests` / `wait_for` are honored end to end
- `capture_game_view` / `capture_scene_view` are returned as MCP image content (base64 PNG)
- Bearer tokens in any returned text are replaced with `[redacted]`
- Speaks MCP protocol versions `2024-11-05` / `2025-03-26` / `2025-06-18`

### `configure mcp` — print a client config snippet

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

Without a pinned project: with exactly one reachable Editor it connects automatically; with several it fails due to ambiguity — append `"--project-path" "<absolute project path>"` to `args` (one `mcpServers` entry per project, no interference).

## Appendix

### Patch mechanism

- Built-in patches are embedded into the exe at compile time (`build.rs`), split by package version: `< 0.7.0-exp.1` → `legacy`, `≥ 0.7.0-exp.1` → `v0.7`; installation dry-runs them first and only applies on success
- Custom patches live in `patchs/custom/` under the working directory, named `modify_unity_local_1.patch`, `modify_unity_local_2.patch`, …, applied in ascending order after the built-ins
- `patchs/system/` is an auto-generated mirror of the embedded patches, rebuilt on every install — do not edit

### Build & release

```bash
cargo build --release              # generic
build-release.bat                  # Windows: one-shot build into releases/
./build-release.sh universal       # macOS: intel / arm / universal / all
```

### Relation to the official Unity CLI

The official [Unity CLI](https://docs.unity.com/en-us/unity-cli/use-unity-cli) covers Editor installation, project creation, version-control setup, and more; controlling the Editor itself requires the separate `com.unity.pipeline` package. This project does — and enhances — exactly that slice (patched one-shot install, instance discovery, command execution, MCP bridging); it does not install Editors, create projects, or integrate Unity accounts / Cloud. Version coverage aims beyond the official CLI: built-in patches make the upstream package run on Editors it does not support out of the box — **Unity 2022.3** is already stably supported.
