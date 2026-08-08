//! GUI 侧的白名单缓存 —— 逻辑本体在 `portreaper_core::Whitelist`，这里只提供
//! 桌面版需要的那层「进程级单例」。
//!
//! 为什么还需要这一层：引擎的 `Whitelist` 是值类型（短命的 CLI/Raycast 进程用
//! 起来才自然），常驻 GUI 需要一个活到退出的实例来承载 `path` 与 `writable`
//! 这些一次性解析出来的状态 —— 这个差异属于**前端的生命周期策略**，不属于引擎。
//!
//! 注意「常驻」指的是**实例**，不是**内容**：`get_all` 每轮都会 `refresh()` 对齐
//! 磁盘。白名单是三个前端共写的共享状态，把内容也缓存住会让外部加的星在桌面版
//! 永不可见（详见 `get_all` 的注释——那是一条误杀路径）。
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
    // 一个脱离磁盘的白名单 —— 读永远为空、写响亮失败，绝不静默假成功。
    WHITELIST.get_or_init(|| Mutex::new(Whitelist::detached()))
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

/// 每轮扫描前重新对齐磁盘 —— 白名单是**跨前端共享状态**，Raycast/CLI 加的星
/// 必须在桌面版的下一轮扫描可见（CLAUDE.md 的不变量）。
///
/// 这里曾经只返回内存快照。写方向由 core 的写前合并兜住了，读方向却一直陈旧：
/// 外部加的星桌面版永远看不到，那一行仍标红、仍计入托盘、**仍留在一键清扫的
/// 目标集里** —— 用户刚收藏的进程被清扫杀掉。真机实测复现（v0.9.0 上架 Raycast
/// 前的跨端 ★ 同步验收）。
///
/// 代价：一次几百字节的 read + serde parse，与同一轮里的 lsof + 两次 ps +
/// launchctl 相比可忽略。缓存本身仍有意义 —— 它省的是 `Whitelist::load` 的
/// 损坏备份等副作用，不是这次读盘。
pub fn get_all() -> Vec<String> {
    let mut wl = lock();
    wl.refresh();
    wl.entries().to_vec()
}

/// 持久化失败（磁盘满/权限/路径被占）会上抛给前端：星标回弹 + 错误横幅，
/// 而不是内存假成功、重启后丢收藏（评审发现）。
pub fn add(key: String) -> Result<(), String> {
    lock().add(key)
}

pub fn remove(key: &str) -> Result<(), String> {
    lock().remove(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 钉住桌面侧那段真实接线：`init` 之后、在本进程没有任何 add/remove 的情况下，
    /// `get_all` 必须看得见另一个前端（CLI / Raycast）写进磁盘的改动。
    ///
    /// 引擎侧的 `Whitelist::refresh` 另有单测；这条钉的是**这里有没有调它** ——
    /// 漏掉这一行，`scan_ports` 每轮拿到的就是进程启动那一刻的陈旧快照：外部加的
    /// 星桌面版永远看不到，那一行仍标红、仍计入托盘、**仍留在一键清扫的目标集里**。
    /// 这个事故在真机上复现过（v0.9.0 上架 Raycast 前的跨端 ★ 同步验收）。
    ///
    /// 本模块是进程级单例，故整个测试二进制里只有这一个测试碰它。
    #[test]
    fn get_all_picks_up_changes_written_by_another_frontend() {
        let dir = std::env::temp_dir().join(format!("portreaper-gui-wl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("whitelist.json");

        init(path.clone());
        add("/gui/own".to_string()).unwrap();
        assert_eq!(get_all(), vec!["/gui/own".to_string()]);

        // 另一个前端进程：自己 load、自己改、退出。本进程全程不 mutate。
        let mut other = Whitelist::load(path.clone());
        other.add("/cli/starred".to_string()).unwrap();
        other.remove("/gui/own").unwrap();

        let seen = get_all();
        assert!(
            seen.contains(&"/cli/starred".to_string()),
            "外部加的星必须在下一轮扫描可见，实际: {seen:?}"
        );
        assert!(
            !seen.contains(&"/gui/own".to_string()),
            "外部取消的星必须同步消失，实际: {seen:?}"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
