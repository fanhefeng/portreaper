//! GUI 侧的白名单缓存 —— 逻辑本体在 `portreaper_core::Whitelist`，这里只提供
//! 桌面版需要的那层「进程级单例」。
//!
//! 为什么还需要这一层：桌面版每 2 秒扫描一次，每轮都要读白名单。引擎的
//! `Whitelist` 是值类型（短命的 CLI/Raycast 进程用起来才自然），常驻 GUI 则
//! 希望它加载一次、活到退出 —— 这个差异属于**前端的生命周期策略**，不属于引擎。
//!
//! 锁：单把 `Mutex`，且从毒化中恢复。恢复策略与 scanner 的 System 锁同源 ——
//! `Whitelist` 的 add/remove 有显式回滚，不存在半写入的结构失效；若不恢复，
//! 持锁 panic 一次会让此后每轮 scan_ports（内部 get_all）在 spawn_blocking 里
//! 跟着 panic，前端永久收到 "scan task failed"（评审发现：拒绝服务比脏读更糟）。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

use portreaper_core::Whitelist;

static WHITELIST: OnceLock<Mutex<Whitelist>> = OnceLock::new();

fn cell() -> &'static Mutex<Whitelist> {
    // init() 未跑（理论上不该发生：lib.rs setup 内、任何命令可达之前调用）时的降级形态：
    // 一个指向空路径的白名单 —— 读永远为空、写响亮失败，绝不静默假成功。
    WHITELIST.get_or_init(|| Mutex::new(Whitelist::empty(PathBuf::new())))
}

fn lock() -> MutexGuard<'static, Whitelist> {
    cell().lock().unwrap_or_else(PoisonError::into_inner)
}

/// 载入白名单文件。由 `lib.rs` 的 setup 调用一次。
pub fn init(path: PathBuf) {
    let loaded = Whitelist::load(path);
    if WHITELIST.set(Mutex::new(loaded.clone())).is_err() {
        // 已被 get_or_init 抢先（降级形态）——用真正载入的内容覆盖它
        *lock() = loaded;
    }
}

pub fn get_all() -> Vec<String> {
    lock().entries().to_vec()
}

/// 持久化失败（磁盘满/权限/路径被占）会上抛给前端：星标回弹 + 错误横幅，
/// 而不是内存假成功、重启后丢收藏（评审发现）。
pub fn add(key: String) -> Result<(), String> {
    lock().add(key)
}

pub fn remove(key: &str) -> Result<(), String> {
    lock().remove(key)
}
