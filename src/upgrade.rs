use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

use crate::i18n::{t, tr};
use crate::install::{download, extract_tarball, http_get_text};

const RELEASE_API_URL: &str =
    "https://api.github.com/repos/koyoter/unity_pipeline_cli/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub fn run() -> Result<()> {
    // 必须最先执行：本次运行不管走哪个分支（已最新/取消/升级），
    // 都该顺手清掉上次升级遗留的 .old，放在任何提前 return 之后永远轮不到。
    clean_stale_old_exe();

    let current = env!("CARGO_PKG_VERSION");
    // 平台标签与 Release 资产名的平台段一致（见 main.rs VERSION_STR 注释）。
    let platform = crate::VERSION_STR
        .split_whitespace()
        .last()
        .unwrap_or_default();

    println!("{}", t("upgrade.checking_release"));
    let release = fetch_latest_release()?;
    if !is_newer(&release.tag_name, current)? {
        println!("{}", tr("upgrade.already_latest", &[&current]));
        return Ok(());
    }
    let latest = release.tag_name.strip_prefix('v').unwrap_or(&release.tag_name);

    println!("{}", tr("upgrade.found_new_version", &[&current, &latest]));
    let body = release.body.as_deref().unwrap_or_default().trim();
    if !body.is_empty() {
        println!("\n{body}\n");
    }

    let asset = pick_asset(&release.assets, platform).ok_or_else(|| {
        let available = release
            .assets
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow!(
            "{}",
            tr("upgrade.no_platform_asset", &[&latest, &platform, &available])
        )
    })?;

    print!("{}", tr("upgrade.confirm_update", &[&latest]));
    io::stdout().flush().ok();
    let mut line = String::new();
    io::stdin().read_line(&mut line).context(t("upgrade.read_input_failed"))?;
    if !matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("{}", tr("upgrade.cancelled", &[&current]));
        return Ok(());
    }

    let exe = std::env::current_exe().context(t("upgrade.locate_current_exe_failed"))?;
    let exe_dir = exe
        .parent()
        .ok_or_else(|| anyhow!("{}", t("upgrade.unknown_exe_dir")))?;
    let exe_name = exe
        .file_name()
        .ok_or_else(|| anyhow!("{}", t("upgrade.unknown_exe_name")))?
        .to_owned();
    let old_path = exe_dir.join(format!("{}.old", exe_name.to_string_lossy()));

    // 临时目录放在 exe 旁边：与 exe 同卷，rename 替换才不会跨盘失败。
    let tmp_dir = exe_dir.join(format!(".upgrade_{}", std::process::id()));
    if tmp_dir.exists() {
        fs::remove_dir_all(&tmp_dir)
            .with_context(|| tr("upgrade.clean_tmp_dir_failed", &[&tmp_dir.display()]))?;
    }
    fs::create_dir_all(&tmp_dir)
        .with_context(|| tr("upgrade.create_tmp_dir_failed", &[&tmp_dir.display()]))?;

    let zip_path = tmp_dir.join(&asset.name);
    println!("{}", tr("upgrade.downloading", &[&asset.name]));
    download(&asset.browser_download_url, &zip_path)
        .with_context(|| tr("upgrade.download_failed", &[&asset.browser_download_url]))?;

    println!("{}", t("upgrade.extracting"));
    let extract_dir = tmp_dir.join("extracted");
    fs::create_dir_all(&extract_dir)
        .with_context(|| tr("upgrade.create_extract_dir_failed", &[&extract_dir.display()]))?;
    extract_tarball(&zip_path, &extract_dir)?;
    let new_exe = extracted_file(&extract_dir, &exe)?;

    // macOS 上若 zip 丢了可执行位，入位前补上，避免升级后跑不起来。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&new_exe, fs::Permissions::from_mode(0o755))
            .with_context(|| tr("upgrade.set_exec_permission_failed", &[&new_exe.display()]))?;
    }

    // Windows 不允许覆盖运行中的 exe，但允许改名让位：exe → exe.old，新 exe 入位。
    fs::rename(&exe, &old_path).with_context(|| {
        tr(
            "upgrade.rename_current_exe_failed",
            &[&exe.display(), &old_path.display()],
        )
    })?;
    if let Err(err) = fs::rename(&new_exe, &exe) {
        // 回滚让位的旧 exe，保持原状。
        let _ = fs::rename(&old_path, &exe);
        let _ = fs::remove_dir_all(&tmp_dir);
        return Err(err).with_context(|| {
            tr(
                "upgrade.place_new_exe_failed",
                &[&new_exe.display(), &exe.display()],
            )
        });
    }
    let _ = fs::remove_dir_all(&tmp_dir);
    // Windows 上运行中的旧 exe 此刻仍被锁，删不掉就留给下次升级时清理。
    let _ = fs::remove_file(&old_path);

    println!("{}", tr("upgrade.updated", &[&latest]));
    Ok(())
}

fn fetch_latest_release() -> Result<Release> {
    let body = http_get_text(RELEASE_API_URL).context(t("upgrade.release_api_request_failed"))?;
    serde_json::from_str(&body)
        .with_context(|| tr("upgrade.parse_release_json_failed", &[&RELEASE_API_URL]))
}

/// 清掉上次升级留下的 `<exe>.old`。Windows 不允许删除运行中的 exe，所以上次
/// 升级结束时它还锁在旧进程镜像上；本次进程不占用它，趁启动删掉。刚退出的
/// 上次升级进程其镜像锁可能尚未释放，重试至多 3s，仍失败则留待下次。
fn clean_stale_old_exe() {
    let Ok(exe) = std::env::current_exe() else { return };
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else { return };
    let stale = dir.join(format!("{}.old", name.to_string_lossy()));
    if !stale.exists() {
        return;
    }
    for _ in 0..12 {
        if fs::remove_file(&stale).is_ok() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}

/// tag 形如 `v0.1.2`；与当前 `CARGO_PKG_VERSION` 做 semver 比较，更新才算新。
fn is_newer(tag: &str, current: &str) -> Result<bool> {
    let latest = tag.strip_prefix('v').unwrap_or(tag);
    let latest = semver::Version::parse(latest)
        .with_context(|| tr("upgrade.parse_tag_failed", &[&tag]))?;
    let current = semver::Version::parse(current)
        .with_context(|| tr("upgrade.parse_current_version_failed", &[&current]))?;
    Ok(latest > current)
}

/// 按平台标签匹配资产：`unity_pipeline_cli-0.1.2-windows-x86_64.tar.gz` 的后缀
/// `-windows-x86_64.tar.gz` 与 `--version` 输出的平台 token 一致。
fn pick_asset<'a>(assets: &'a [Asset], platform: &str) -> Option<&'a Asset> {
    let suffix = format!("-{platform}.tar.gz");
    assets.iter().find(|a| a.name.ends_with(&suffix))
}

/// tar.gz 里只有一个根级文件（CI 只打包单个 exe），取出它。
fn extracted_file(dir: &Path, exe: &Path) -> Result<PathBuf> {
    let entries = fs::read_dir(dir)
        .with_context(|| tr("upgrade.read_dir_failed", &[&dir.display()]))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry?.path();
        if path.is_file() {
            files.push(path);
        }
    }
    match files.as_slice() {
        [only] => Ok(only.clone()),
        _ => Err(anyhow!(
            "{}",
            tr(
                "upgrade.ambiguous_extracted_file",
                &[&files.len(), &exe.display()]
            )
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_compares_semver() {
        assert!(is_newer("v0.2.0", "0.1.2").unwrap());
        assert!(is_newer("v0.2.0-beta.1", "0.1.2").unwrap());
        // 开发版（CI 手动触发的 0.0.0-dev.N）对正式版总是可更新。
        assert!(is_newer("v0.1.2", "0.0.0-dev.7").unwrap());
        assert!(!is_newer("v0.1.2", "0.1.2").unwrap());
        assert!(!is_newer("v0.1.1", "0.1.2").unwrap());
        assert!(is_newer("not-a-version", "0.1.2").is_err());
    }

    #[test]
    fn pick_asset_matches_platform_suffix() {
        let assets: Vec<Asset> = [
            "unity_pipeline_cli-0.1.2-windows-x86_64.tar.gz",
            "unity_pipeline_cli-0.1.2-macos-x86_64.tar.gz",
            "unity_pipeline_cli-0.1.2-macos-arm64.tar.gz",
            "unity_pipeline_cli-0.1.2-macos-universal.tar.gz",
        ]
        .into_iter()
        .map(|name| Asset {
            name: name.to_owned(),
            browser_download_url: String::new(),
        })
        .collect();
        let name = |a: Option<&Asset>| a.map(|a| a.name.clone());
        assert_eq!(
            name(pick_asset(&assets, "windows-x86_64")),
            Some("unity_pipeline_cli-0.1.2-windows-x86_64.tar.gz".to_owned())
        );
        assert_eq!(
            name(pick_asset(&assets, "macos-arm64")),
            Some("unity_pipeline_cli-0.1.2-macos-arm64.tar.gz".to_owned())
        );
        assert!(pick_asset(&assets, "linux-x86_64").is_none());
    }
}
