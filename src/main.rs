use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod command;
mod configure;
mod editor;
mod http;
mod install;
mod json;
mod mcp;
mod pipeline;
mod process;
mod projects;

#[derive(Parser)]
#[command(
    name = "unity",
    about = "Unity offline CLI (Rust port)",
    version,
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    command: TopCommand,
}

#[derive(Subcommand)]
enum TopCommand {
    /// List Unity projects currently open in the Editor.
    Projects {
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Launch a stdio MCP server bridging agents to a running Unity Editor.
    Mcp {
        /// Target a specific Unity project by absolute path.
        #[arg(long)]
        project_path: Option<String>,
    },
    /// List pipeline commands, or execute one on a running Unity Editor.
    #[command(alias = "cmd", alias = "request")]
    Command {
        /// Pipeline command to execute. Omit to list all available commands.
        name: Option<String>,
        /// Arguments forwarded to the pipeline command (e.g. `--code "1+1"`).
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        /// Target a specific Unity project by absolute path.
        #[arg(long)]
        project_path: Option<String>,
        /// Request timeout in seconds.
        #[arg(long, default_value_t = 30)]
        timeout: u64,
        /// Emit machine-readable JSON instead of the human summary.
        #[arg(long)]
        json: bool,
    },
    /// Download a `com.unity.pipeline` release from the UPM registry and unpack it.
    Install {
        /// Skip the picker; install this exact version.
        #[arg(long)]
        version: Option<String>,
        /// Skip the picker; install the registry's `dist-tags.latest`.
        #[arg(long)]
        latest: bool,
        /// Reuse an existing tarball in `downloads/` instead of re-fetching.
        #[arg(long)]
        keep_cache: bool,
    },
    /// Print client-agnostic configuration snippets (currently: `mcp`).
    Configure {
        #[command(subcommand)]
        target: ConfigureTarget,
    },
}

#[derive(Subcommand)]
enum ConfigureTarget {
    /// Print a generic MCP `mcpServers` config entry for this binary.
    Mcp,
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(c) => c,
        Err(e) => {
            let _ = e.print();
            // 如果用户没有传递任何参数（通常是双击运行的情况），则暂停以防窗口一闪而过
            if std::env::args().count() <= 1 {
                println!("\n按回车键退出...");
                let mut buf = String::new();
                let _ = std::io::stdin().read_line(&mut buf);
            }
            std::process::exit(e.exit_code());
        }
    };

    match cli.command {
        TopCommand::Projects { json } => match projects::run(json) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Error: {err:#}");
                ExitCode::from(1)
            }
        },
        TopCommand::Mcp { project_path } => match mcp::run(mcp::Options { project_path }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Error: {err:#}");
                ExitCode::from(1)
            }
        },
        TopCommand::Command {
            name,
            args,
            project_path,
            timeout,
            json,
        } => {
            let result = command::run(command::Options {
                name,
                args,
                project_path,
                timeout_seconds: timeout,
                as_json: json,
            });
            match result {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("Error: {err:#}");
                    ExitCode::from(1)
                }
            }
        }
        TopCommand::Install {
            version,
            latest,
            keep_cache,
        } => match install::run(install::Options {
            version,
            latest,
            keep_cache,
        }) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("Error: {err:#}");
                ExitCode::from(1)
            }
        },
        TopCommand::Configure { target } => match target {
            ConfigureTarget::Mcp => match configure::print_mcp_config(None) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("Error: {err:#}");
                    ExitCode::from(1)
                }
            },
        },
    }
}
