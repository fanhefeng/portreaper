//! 分环境目录解析 —— **不依赖 Tauri**，供任何前端（GUI 壳、CLI、Raycast）共用。
//!
//! # 为什么必须逐字节复刻 Tauri 的算法
//!
//! 白名单文件是所有前端**共享**的状态：用户在 Raycast 里加的星标，桌面版下一轮
//! 扫描必须立刻看见。只要这里算出的目录与 `app.path().app_config_dir()` 差一个
//! 字符，两边就各写各的 whitelist.json，症状是「星标加了但对面看不到」——
//! 一个不会报错、只会让人怀疑自己记错了的 bug。
//!
//! 因此下面每个函数都对齐 tauri 2.11 `path/desktop.rs` 的实现（已核对源码，
//! 非凭记忆）：
//!
//! | 本模块        | Tauri                  | 实现                                          |
//! |---------------|------------------------|-----------------------------------------------|
//! | `config_dir`  | `app_config_dir()`     | `dirs::config_dir()/{id}`                     |
//! | `data_dir`    | `app_data_dir()`       | `dirs::data_dir()/{id}`                       |
//! | `cache_dir`   | `app_cache_dir()`      | `dirs::cache_dir()/{id}`                      |
//! | `log_dir`     | `app_log_dir()`        | macOS `~/Library/Logs/{id}`；其余 `dirs::data_local_dir()/{id}/logs` |
//! | `temp_dir`    | `temp_dir()` + join    | `std::env::temp_dir()/{id}`                   |
//!
//! 依赖同一个 `dirs` crate 且 major 与 tauri 一致（6），cargo 会把两者归并成
//! 同一份实现 —— 基目录的语义差异从源头上不可能出现。**升级 tauri 时若它换掉
//! `dirs` 的 major，这里必须同步**，`src-tauri` 启动时的一致性断言会当场喊出来。
//!
//! # 分环境隔离
//!
//! 与旧的 `src-tauri/src/paths.rs` 同一策略，原样保留：debug 构建把数据放进
//! 各基目录下的 `dev/` 子目录，release 直接用基目录。用编译期
//! `cfg(debug_assertions)` 而非运行期开关。
//!
//! 这条规则在 CLI 上自动成立且正是我们想要的：release 编译的 CLI 指向 prod
//! 目录（与安装版 GUI 共享白名单），`cargo run` 出来的 debug CLI 指向 `dev/`
//! （与 `pnpm tauri dev` 共享）。别把它当成巧合「修」掉。

use std::path::PathBuf;

/// bundle identifier —— 必须与 `src-tauri/tauri.conf.json` 的 `identifier` 一致。
/// 由 `scripts/check-paths-parity.mjs` 静态校验，并在 GUI 启动时运行时断言。
pub const APP_IDENTIFIER: &str = "com.fhf.portreaper";

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
pub fn config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| scoped(d.join(APP_IDENTIFIER)))
}

/// 分环境后的数据（文件存储）目录。
pub fn data_dir() -> Option<PathBuf> {
    dirs::data_dir().map(|d| scoped(d.join(APP_IDENTIFIER)))
}

/// 分环境后的缓存目录（可重建的临时性数据，OS 可能在空间紧张时回收）。
pub fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| scoped(d.join(APP_IDENTIFIER)))
}

/// 分环境后的日志目录。
///
/// 平台分叉照抄 Tauri：macOS 走 `~/Library/Logs/{id}`（系统惯例，Console.app
/// 能直接看到），其余平台走 `data_local_dir()/{id}/logs`。**注意 macOS 用的是
/// `home_dir` 而非 `data_local_dir`** —— 写成后者会把日志丢进
/// `~/Library/Application Support`，与 GUI 分家。
pub fn log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let base = dirs::home_dir().map(|d| d.join("Library/Logs").join(APP_IDENTIFIER));

    #[cfg(not(target_os = "macos"))]
    let base = dirs::data_local_dir().map(|d| d.join(APP_IDENTIFIER).join("logs"));

    base.map(scoped)
}

/// 分环境后的应用专属临时目录。
///
/// `std::env::temp_dir()` 是所有进程共享的系统临时根（macOS `$TMPDIR`、
/// Windows `%TEMP%`），故先落到 bundle id 专属子目录、再按环境隔离 ——
/// 否则「打开临时目录」菜单会把整个系统 temp 摊开给用户。
pub fn temp_dir() -> PathBuf {
    scoped(std::env::temp_dir().join(APP_IDENTIFIER))
}

/// 白名单文件路径（配置目录下的 whitelist.json）。
/// 所有前端共享同一份 —— 这是「Raycast 加星，GUI 立刻可见」的物理保证。
pub fn whitelist_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("whitelist.json"))
}

/// 日志文件主名（不含扩展名，tauri-plugin-log 会追加 `.log`）。
/// 即便日后两个环境的目录被指到一处，文件名也带环境后缀、不会互相覆盖 —— 双保险。
pub fn log_file_name() -> String {
    format!("portreaper-{}", env_label())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 结构性断言：每个目录都以 identifier 命名的段落结尾（或其 `dev/` 子目录）。
    /// 真正防漂移的是 `src-tauri` 启动时与 Tauri 结果的逐一比对 —— 那里才有
    /// 权威答案；此处只保证本模块自身的拼装没有笔误。
    #[test]
    fn dirs_are_identifier_scoped() {
        let expect_tail = |p: PathBuf| {
            let s = p.to_string_lossy().to_string();
            assert!(
                s.contains(APP_IDENTIFIER),
                "路径必须包含 bundle identifier: {s}"
            );
            if cfg!(debug_assertions) {
                assert!(s.ends_with("dev"), "debug 构建必须落在 dev/ 子目录: {s}");
            } else {
                assert!(!s.ends_with("dev"), "release 构建不应有 dev/ 子目录: {s}");
            }
        };

        // 这些在 CI 容器里也应当有值；为 None 说明环境异常，让测试响亮失败
        expect_tail(config_dir().expect("config_dir"));
        expect_tail(data_dir().expect("data_dir"));
        expect_tail(cache_dir().expect("cache_dir"));
        expect_tail(log_dir().expect("log_dir"));
        expect_tail(temp_dir());
    }

    /// macOS 的日志目录必须在 ~/Library/Logs 下，而非 Application Support ——
    /// 写错会让 CLI/GUI 的日志分家（照抄 Tauri 时最容易搞混的一处）。
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_log_dir_is_library_logs() {
        let p = log_dir().expect("log_dir").to_string_lossy().to_string();
        assert!(p.contains("Library/Logs"), "got: {p}");
        assert!(!p.contains("Application Support"), "got: {p}");
    }

    #[test]
    fn whitelist_lives_under_config_dir() {
        let wl = whitelist_path().expect("whitelist_path");
        assert_eq!(wl.file_name().unwrap(), "whitelist.json");
        assert_eq!(wl.parent().unwrap(), config_dir().expect("config_dir"));
    }

    #[test]
    fn log_file_name_carries_env_label() {
        assert_eq!(log_file_name(), format!("portreaper-{}", env_label()));
    }
}
