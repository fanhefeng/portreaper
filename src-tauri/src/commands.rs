use std::sync::{Mutex, PoisonError};

use portreaper_core::{kill, KillError, ProcessEntry, Scanner};
use tauri::{AppHandle, Manager};

use crate::whitelist;

/// 常驻扫描器 —— 由 `lib.rs` 注册为 Tauri 托管状态。
///
/// **必须跨轮询存活**：Windows 的 CPU 百分比是 sysinfo 两次 refresh 之间的增量，
/// 前端每 2 秒的轮询正是它的采样区间。若改成每次新建 Scanner，Windows 上每一行
/// 的 CPU 都会永远是 0%（拆分前这份状态是 scanner 内部的进程级 static，语义相同、
/// 只是不可见）。
pub struct ScannerState(pub Mutex<Scanner>);

/// async + spawn_blocking（评审发现）：Tauri 2 的非 async 命令在主线程执行，
/// scan() 每 2s shell 出 lsof + 两次 ps + launchctl（几十到几百毫秒）会周期性
/// 阻塞事件循环（托盘/窗口事件卡顿）。挪到阻塞线程池，主线程零占用。
#[tauri::command]
pub async fn scan_ports(app: AppHandle) -> Result<Vec<ProcessEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<ScannerState>();
        // try_lock 而非 lock（评审发现）：前端 10s 超时后仍每 2s 轮询，若某轮
        // scan 里 lsof 永久挂死（网络挂载卷等），排队 lock 会让阻塞池线程每
        // ~10s 新增一个、直至 tokio 阻塞池 512 上限耗尽 —— 那之后 kill_process
        // 的 spawn_blocking 排队永不执行。拿不到锁说明上一轮还在跑，直接拒绝
        // 本轮：前端把 ERR_SCAN_BUSY 展示为普通扫描错误，下一轮轮询自动重试。
        let mut scanner = match state.0.try_lock() {
            Ok(guard) => guard,
            // 毒化恢复：scan 中途 panic 一次不应让后续每轮轮询永久 panic（前端
            // 表现为永远 ERR_SCAN_TIMEOUT）。Scanner 内部只是采集缓存，半更新
            // 状态可安全续用 —— 拆分前这段恢复逻辑在 windows.rs 的 System 锁上。
            Err(std::sync::TryLockError::Poisoned(p)) => p.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                return Err("ERR_SCAN_BUSY: previous scan still running".to_string());
            }
        };
        let wl = whitelist::get_all();
        Ok(scanner.scan(&wl))
    })
    .await
    .map_err(|e| format!("scan task failed: {e}"))?
}

/// 终止进程。`start_unix` 是扫描时捕获的创建时间 —— kill 前重新核对，
/// 防止 scan 与点击之间 PID 被复用导致误杀（Windows 复用尤其激进）。
/// async 理由同 scan_ports（macOS 分支 shell 出 ps + kill 两个子进程）。
///
/// 错误直接返回引擎的 `KillError`：Tauri 按 serde 形态 `{code, message?}` 过 IPC，
/// 与 CLI 写给 Raycast 的 stderr **同一个值、同一套 code**（v0.9.0 统一，此前桌面
/// 侧多一层 `ERR_*:` 字符串降级）。前端按 `code` 分派本地化，不解析人类文案。
///
/// spawn_blocking 自身的 join 失败（panic / 运行时关停）无 kill 语义，归入
/// `Os` 变体——前端会把它当无语义系统错误原样展示，正确：这不是「杀不掉」的
/// 四种已知原因之一，冒充任何一个都会给出误导性的处置建议。
#[tauri::command]
pub async fn kill_process(pid: u32, force: bool, start_unix: Option<u64>) -> Result<(), KillError> {
    tauri::async_runtime::spawn_blocking(move || kill(pid, force, start_unix))
        .await
        .map_err(|e| KillError::Os {
            message: format!("kill task failed: {e}"),
        })?
}

/// 前端平台感知（驱动平台分叉的文案与按钮布局），不引入额外 JS 插件。
#[tauri::command]
pub fn get_platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else {
        "windows"
    }
}

/// 持久化失败（磁盘满/权限/路径被占）会上抛给前端：星标回弹 + 错误横幅，
/// 而不是内存假成功、重启后丢收藏（评审发现）。
#[tauri::command]
pub fn add_whitelist(key: String) -> Result<(), String> {
    whitelist::add(key)
}

#[tauri::command]
pub fn remove_whitelist(key: String) -> Result<(), String> {
    whitelist::remove(&key)
}

/// 托盘计数展示：macOS 用菜单栏标题文本；Windows 通知区无标题，用 tooltip。
#[tauri::command]
pub fn update_tray_title(
    app: tauri::AppHandle,
    count: u32,
    suspect_count: u32,
) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id("main-tray") {
        #[cfg(target_os = "macos")]
        {
            let title = if suspect_count > 0 {
                format!("{} ⚠", count)
            } else {
                format!("{}", count)
            };
            tray.set_title(Some(title.as_str()))
                .map_err(|e| e.to_string())?;
        }
        #[cfg(windows)]
        {
            // 毒化恢复：语言只是一个 &'static str，持锁 panic 不可能让它处于
            // 半更新的无效状态。不恢复的话，一次 panic 就让托盘计数此后永久
            // 报错 —— 与 scan_ports / whitelist 的锁同一套取舍（评审发现）。
            let lang = app
                .try_state::<crate::TrayLang>()
                .map(|l| *l.0.lock().unwrap_or_else(PoisonError::into_inner))
                .unwrap_or("en");
            let tooltip = match (lang, suspect_count > 0) {
                ("zh", true) => {
                    format!("Portreaper — {} 端口，{} 疑似僵尸 ⚠", count, suspect_count)
                }
                ("zh", false) => format!("Portreaper — {} 端口", count),
                (_, true) => format!("Portreaper — {} ports, {} suspects ⚠", count, suspect_count),
                (_, false) => format!("Portreaper — {} ports", count),
            };
            tray.set_tooltip(Some(tooltip.as_str()))
                .map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// 前端切换语言时同步托盘菜单文案与 tooltip 语言。
#[tauri::command]
pub fn set_tray_language(app: tauri::AppHandle, lang: String) -> Result<(), String> {
    let lang: &'static str = if lang.to_lowercase().starts_with("zh") {
        "zh"
    } else {
        "en"
    };
    if let Some(state) = app.try_state::<crate::TrayLang>() {
        // 毒化恢复同上：语言切换失败一次就永久切不动，比脏读糟得多
        *state.0.lock().unwrap_or_else(PoisonError::into_inner) = lang;
    }
    if let Some(items) = app.try_state::<crate::TrayMenuItems>() {
        let (show, quit) = crate::tray_texts(lang);
        items.show.set_text(show).map_err(|e| e.to_string())?;
        items.dir.set_lang(lang)?;
        items.quit.set_text(quit).map_err(|e| e.to_string())?;
    }
    // macOS 应用菜单的 ⌘Q 替代项 + 「打开目录」菜单（顶部应用菜单栏那份）同步语言
    #[cfg(target_os = "macos")]
    if let Some(items) = app.try_state::<crate::AppMenuItems>() {
        items
            .quit_to_tray
            .set_text(crate::quit_to_tray_text(lang))
            .map_err(|e| e.to_string())?;
        items.dir.set_lang(lang)?;
    }
    Ok(())
}
