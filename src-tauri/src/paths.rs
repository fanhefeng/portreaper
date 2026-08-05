//! 目录解析的 **Tauri 侧适配**。
//!
//! 算法本体已下沉到 `portreaper_core::paths`（不依赖 Tauri，CLI / Raycast 共用）。
//! 本模块只做两件事：
//!
//! 1. 把引擎的 `Option<PathBuf>` 适配成 Tauri 命令惯用的 `tauri::Result`；
//! 2. **一致性断言**（`assert_matches_tauri`）—— 逐一比对引擎自解析的结果与
//!    `app.path().app_*_dir()`，不一致就喊出来。
//!
//! 第 2 条是整次拆分里最要命的一环。白名单文件是所有前端共享的状态：用户在
//! Raycast 里加的星标，桌面版下一轮扫描必须立刻看见。两边的目录算法只要差一个
//! 字符，就会各写各的 whitelist.json —— 症状是「星标加了但对面看不到」，不报错、
//! 不崩溃，只让人怀疑自己记错了。所以宁可在启动时吵闹。
//!
//! 断言放在**运行时**而非单元测试：Tauri 的路径解析需要一个活的 AppHandle，
//! 而 mock app 会拉起窗口，在 headless CI 上并不可靠。真实启动是唯一能同时拿到
//! 两侧答案的地方。debug 构建直接 panic（开发时立刻发现），release 只记
//! `log::error!` —— 用户的进程管理器不该因为日志目录算错就打不开。
//!
//! 分环境隔离（debug → `dev/` 子目录）的语义见 `portreaper_core::paths`。

use std::path::PathBuf;

use tauri::{AppHandle, Manager};

pub use portreaper_core::paths::{env_label, log_file_name};

fn required(opt: Option<PathBuf>, what: &str) -> tauri::Result<PathBuf> {
    match opt {
        Some(p) => Ok(p),
        None => {
            log::error!("could not resolve {what} directory");
            Err(tauri::Error::UnknownPath)
        }
    }
}

/// 分环境后的配置目录（白名单 whitelist.json 落此处）。
pub fn config_dir(_app: &AppHandle) -> tauri::Result<PathBuf> {
    required(portreaper_core::paths::config_dir(), "config")
}

/// 分环境后的日志目录（tauri-plugin-log 的 Folder target 指向此处）。
pub fn log_dir(_app: &AppHandle) -> tauri::Result<PathBuf> {
    required(portreaper_core::paths::log_dir(), "log")
}

/// 分环境后的缓存目录（可重建的临时性数据，OS 可能在空间紧张时回收）。
pub fn cache_dir(_app: &AppHandle) -> tauri::Result<PathBuf> {
    required(portreaper_core::paths::cache_dir(), "cache")
}

/// 分环境后的数据（文件存储）目录。
pub fn data_dir(_app: &AppHandle) -> tauri::Result<PathBuf> {
    required(portreaper_core::paths::data_dir(), "data")
}

/// 分环境后的应用专属临时目录。
pub fn temp_dir(_app: &AppHandle) -> tauri::Result<PathBuf> {
    Ok(portreaper_core::paths::temp_dir())
}

/// 启动时校验：引擎自解析的目录必须与 Tauri 的解析逐字节相同。
///
/// 覆盖两类漂移：
/// - **identifier 漂移** —— `tauri.conf.json` 改了 bundle id 而 core 的常量没跟；
/// - **算法漂移** —— 升级 tauri 后它换了 `dirs` 的 major，或改了某个目录的平台分叉
///   （macOS 的日志目录走 `home_dir()/Library/Logs` 而非 data_local_dir，是最容易
///   被抄错的一处）。
///
/// temp_dir 不在比对之列：Tauri 侧是 `std::env::temp_dir()` 再由本项目 join
/// identifier，两边同源，没有独立的第三方实现可供背离。
pub fn assert_matches_tauri(app: &AppHandle) {
    let identifier = &app.config().identifier;
    if identifier != portreaper_core::paths::APP_IDENTIFIER {
        report(&format!(
            "bundle identifier 漂移：tauri.conf.json = {identifier}，\
             portreaper_core::paths::APP_IDENTIFIER = {}",
            portreaper_core::paths::APP_IDENTIFIER
        ));
        return; // identifier 已经不一致，逐目录比对只会刷屏
    }

    let cases: [(&str, Option<PathBuf>, tauri::Result<PathBuf>); 4] = [
        (
            "config",
            portreaper_core::paths::config_dir(),
            app.path().app_config_dir(),
        ),
        (
            "data",
            portreaper_core::paths::data_dir(),
            app.path().app_data_dir(),
        ),
        (
            "cache",
            portreaper_core::paths::cache_dir(),
            app.path().app_cache_dir(),
        ),
        (
            "log",
            portreaper_core::paths::log_dir(),
            app.path().app_log_dir(),
        ),
    ];

    for (name, ours, theirs) in cases {
        // Tauri 侧是未分环境的基目录，补上同样的 dev/ 作用域再比
        let theirs = theirs.map(portreaper_core::paths::scoped);
        match (ours, theirs) {
            (Some(a), Ok(b)) if a == b => {}
            (a, b) => report(&format!(
                "{name} 目录漂移：portreaper_core = {a:?}，tauri = {b:?}"
            )),
        }
    }
}

fn report(msg: &str) {
    log::error!("[paths] {msg}");
    // 开发期直接炸掉：这类漂移一旦流到用户手里，表现是「白名单莫名其妙分家」，
    // 排查成本远高于启动即失败。release 只记录，不让用户打不开进程管理器。
    #[cfg(debug_assertions)]
    panic!("[paths] {msg}");
}
