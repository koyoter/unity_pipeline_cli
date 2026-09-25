//! 极简 i18n:locale 文案放在 `locales/{en,zh}.json`,编译期嵌入执行文件。
//! 检测链:`language` 指令保存的设置 > LANGUAGE/LC_ALL/LANG 环境变量 > 系统 UI 语言 > `en`。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

const EN_JSON: &str = include_str!("../locales/en.json");
const ZH_JSON: &str = include_str!("../locales/zh.json");

/// 支持的语言代码,首个为兜底语言。
pub const AVAILABLE: [&str; 2] = ["en", "zh"];

struct Tables {
    en: HashMap<String, String>,
    zh: HashMap<String, String>,
}

static TABLES: OnceLock<Tables> = OnceLock::new();
static ACTIVE: OnceLock<&'static str> = OnceLock::new();

fn tables() -> &'static Tables {
    TABLES.get_or_init(|| Tables {
        en: serde_json::from_str(EN_JSON).expect("locales/en.json 解析失败"),
        zh: serde_json::from_str(ZH_JSON).expect("locales/zh.json 解析失败"),
    })
}

/// 启动时调用一次,确定本次运行的界面语言。
pub fn init() {
    let _ = ACTIVE.set(detect());
}

/// 当前生效的语言代码。
pub fn current() -> &'static str {
    ACTIVE.get().copied().unwrap_or("en")
}

/// 查表取无参数文案;当前语言缺失时回退英文,再缺失原样返回 key。
pub fn t<'a>(key: &'a str) -> &'a str {
    let tb = tables();
    let primary = match current() {
        "zh" => &tb.zh,
        _ => &tb.en,
    };
    primary
        .get(key)
        .or_else(|| tb.en.get(key))
        .map(String::as_str)
        .unwrap_or(key)
}

/// 查表并替换 `{0}` `{1}`… 占位符,`args` 顺序对应编号。
pub fn tr(key: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut s = t(key).to_string();
    for (i, arg) in args.iter().enumerate() {
        s = s.replace(&format!("{{{i}}}"), &arg.to_string());
    }
    s
}

/// 校验并持久化语言设置,返回规范化后的代码。
pub fn set(code: &str) -> anyhow::Result<&'static str> {
    let Some(lang) = normalize(code) else {
        anyhow::bail!("{}", tr("language.invalid", &[&code, &AVAILABLE.join(", ")]));
    };
    let dir = config_dir().ok_or_else(|| anyhow::anyhow!("{}", t("language.no_config_dir")))?;
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("language.txt"), lang)?;
    Ok(lang)
}

fn detect() -> &'static str {
    if let Some(lang) = saved().and_then(|s| normalize(&s)) {
        return lang;
    }
    for var in ["LANGUAGE", "LC_ALL", "LANG"] {
        if let Ok(v) = std::env::var(var) {
            if let Some(lang) = normalize(&v) {
                return lang;
            }
        }
    }
    system_lang().unwrap_or("en")
}

/// `zh_CN.UTF-8` → `zh`,`en_US` → `en`,其余返回 None 继续向下探测。
fn normalize(code: &str) -> Option<&'static str> {
    let c = code.trim().to_ascii_lowercase();
    if c.starts_with("zh") {
        Some("zh")
    } else if c.starts_with("en") {
        Some("en")
    } else {
        None
    }
}

#[cfg(windows)]
fn system_lang() -> Option<&'static str> {
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetUserDefaultUILanguage() -> u16;
    }
    // SAFETY: 无参数、无副作用,可随时调用
    let langid = unsafe { GetUserDefaultUILanguage() };
    // LANGID 低 10 位是主语言:0x04 中文,0x09 英文
    match langid & 0x3ff {
        0x04 => Some("zh"),
        0x09 => Some("en"),
        _ => None,
    }
}

// ponytail: macOS 未读 AppleLanguages(需 objc 调用),终端用户有 LANG 或可手动 `unity language`
#[cfg(not(windows))]
fn system_lang() -> Option<&'static str> {
    None
}

fn saved() -> Option<String> {
    let dir = config_dir()?;
    std::fs::read_to_string(dir.join("language.txt"))
        .ok()
        .map(|s| s.trim().to_string())
}

#[cfg(windows)]
fn config_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("unity_pipeline_cli"))
}

#[cfg(not(windows))]
fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support")
    } else if let Some(x) = std::env::var_os("XDG_CONFIG_HOME") {
        PathBuf::from(x)
    } else {
        PathBuf::from(std::env::var_os("HOME")?).join(".config")
    };
    Some(base.join("unity_pipeline_cli"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_share_the_same_key_set() {
        let tb = tables();
        let mut en: Vec<_> = tb.en.keys().collect();
        let mut zh: Vec<_> = tb.zh.keys().collect();
        en.sort_unstable();
        zh.sort_unstable();
        assert_eq!(en, zh, "en/zh 键集合不一致");
    }

    #[test]
    fn placeholders_are_substituted() {
        let out = tr("language.invalid", &[&"xx", &"en, zh"]);
        assert!(!out.contains('{'), "占位符未替换: {out}");
        assert!(out.contains("xx"));
    }
}
