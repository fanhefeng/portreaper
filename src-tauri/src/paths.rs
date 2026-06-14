//! 分环境目录解析 —— 开发版与生产版的所有持久化数据彻底隔离，互不污染。
//!
//! 隔离用编译期 `cfg(debug_assertions)` 而非运行期开关：`pnpm tauri dev` 产出的是
//! debug 构建、安装包是 release 构建，二者天然对应「开发 / 生产」，无需任何环境变量。
//! debug 构建把数据放进各 Tauri 基目录下的 `dev/` 子目录（白名单、日志……），
//! release 直接用基目录。于是开发时随手测试加的白名单、刷出的报错日志，绝不会
//! 混进日常使用的正式版数据里（反之亦然）。
//!
//! 注：webview 的 localStorage（语言偏好）已天然按 origin 隔离（dev = localhost:1420，
//! prod = tauri://localhost），WKWebView 缓存同理 —— 本模块只需管 Rust 侧自建的目录。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

/// debug 构建的隔离子目录名；release 为 `None`（直接用基目录）。
#[cfg(debug_assertions)]
const ENV_SUBDIR: Option<&str> = Some("dev");
#[cfg(not(debug_assertions))]
const ENV_SUBDIR: Option<&str> = None;

/// 当前环境的人类可读标签（用于日志首行 / 文件名）。
pub fn env_label() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        "prod"
    }
}

fn scoped(base: PathBuf) -> PathBuf {
    match ENV_SUBDIR {
        Some(sub) => base.join(sub),
        None => base,
    }
}

/// 分环境后的配置目录（白名单 whitelist.json 落此处）。
pub fn config_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(scoped(app.path().app_config_dir()?))
}

/// 分环境后的日志目录（tauri-plugin-log 的 Folder target 指向此处）。
pub fn log_dir(app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(scoped(app.path().app_log_dir()?))
}

/// 日志文件主名（不含扩展名，tauri-plugin-log 追加 `.log`）。
/// 即便日后两个环境的目录被指到一处，文件名也带环境后缀、不会互相覆盖 —— 双保险。
pub fn log_file_name() -> String {
    format!("portreaper-{}", env_label())
}
