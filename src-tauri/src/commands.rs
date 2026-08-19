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
        // try_state 而非 state（评审发现）：`Manager::state` 在类型尚未 manage 时
        // 直接 panic，而窗口在 `Builder::build()` 时就已创建并开始加载 —— webview
        // 抢在 setup 跑到 `app.manage(ScannerState(..))` 之前发出首个 scan_ports 是
        // 有窗口的。那一下 panic 会经 JoinError 变成一条不可行动的 "scan task failed"，
        // 外加一份 backtrace 写进 bootstrap 日志。概率低，但换成可重试的错误零成本，
        // 且同文件的 update_tray_title / set_tray_language 本来就是这么写的。
        let Some(state) = app.try_state::<ScannerState>() else {
            return Err("ERR_SCAN_BUSY: scanner not ready yet".to_string());
        };
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
        // 采集失败原样上抛：前端的四态空状态里有一条「扫描失败 + 重试」分支在等它，
        // 而返回空表会被渲染成「这台机器很干净」（引擎侧注释详述）。
        scanner.scan(&wl)
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
    tauri::async_runtime::spawn_blocking(move || {
        let result = kill(pid, force, start_unix);
        // 审计日志：kill 是本应用唯一不可撤销的操作，而在此之前它一个字都不落盘 ——
        // 成功的那些在界面上无声消失（一键清扫可以一次杀掉 N 个），事后想问
        // 「我刚才到底杀了什么」，UI 和日志里都查不到。
        //
        // **只记 pid / 信号 / 结果，不记 app_label 或命令行**：那两样可能含用户的
        // 项目目录名，与 README 承诺的「不上报任何进程信息」同向 —— 日志虽然只在
        // 本机，但它是用户会主动贴进 issue 的东西。
        // 两条路径的字段集必须一致：pid + 信号 + 结果。`force` 就是这一层能表达的
        // 「信号」（macOS 是 SIGKILL / SIGTERM，Windows 只有一种终止方式，具体由
        // 引擎决定，这里不冒充知道信号名）。`start_unix` 刻意不记 —— 它是防误杀的
        // 输入，不是审计事实，事后也无从据它行动。
        let signal = if force { "force" } else { "graceful" };
        match &result {
            Ok(()) => log::info!("kill pid={pid} signal={signal} -> ok"),
            Err(e) => log::warn!("kill pid={pid} signal={signal} -> {e}"),
        }
        result
    })
    .await
    .map_err(|e| {
        // join 失败（panic / 运行时关停）在 IPC 上会被前端展示，但落不了盘 ——
        // 而这恰恰是最需要事后追查的一类失败
        log::error!("kill task failed pid={pid}: {e}");
        KillError::Os {
            message: format!("kill task failed: {e}"),
        }
    })?
}

/// 打开日志目录 —— 与托盘菜单的 `open-log-dir` 同一个动作，只是多一个前端入口。
///
/// 存在的理由是 ErrorBoundary：渲染崩溃时窗口里只剩兜底页，托盘菜单固然还在，
/// 但让一个刚看到崩溃页的用户去菜单栏里翻，等于没有入口。
///
/// 这是本 crate 自己的 `#[tauri::command]`，**不经过 capabilities 的 ACL**
/// （那份白名单管的是核心/插件命令），故 `security-config.test.ts` 的精确断言
/// 一个字都不用改。
#[tauri::command]
pub fn open_log_dir(app: tauri::AppHandle) {
    crate::open_app_dir(&app, crate::paths::log_dir());
}

/// 外观设置（settings.ts appearance）落到原生窗口 chrome：标题栏等跟随
/// webview 内容换肤，否则浅色内容配深色标题栏。CSS 换肤由前端 data-theme
/// 自理，这里只同步原生层。"system" / 未知值 → None（跟随 OS）。
/// 本 crate 自己的命令，不经 capabilities ACL（同 open_log_dir 的注释）。
#[tauri::command]
pub fn set_window_theme(app: tauri::AppHandle, theme: String) {
    let theme = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    };
    app.set_theme(theme);
}

/// 前端平台感知（驱动平台分叉的文案与按钮布局），不引入额外 JS 插件。
///
/// 推导本身在引擎里（`portreaper_core::platform_name`，三个前端共用一份）；
/// 这里只做**本前端的展示收窄**：把 `unknown` 折成 `windows`。理由是前端的
/// `Os` 联合只有两个成员，而 Windows 那套语义在任何平台都安全 —— 单一
/// Terminate 按钮，`force` 参数被引擎忽略，不会凭空给出一个不存在的
/// 「温和/强制」区分。本项目只构建 macOS 与 Windows，这一支实际不可达，
/// 写出来是为了让收窄成为一个显式决定，而不是第二份 cfg 判断（评审发现：
/// 此处曾与 CLI 各写一份，且第三分支的取值还不一致）。
#[tauri::command]
pub fn get_platform() -> &'static str {
    match portreaper_core::platform_name() {
        "macos" => "macos",
        _ => "windows",
    }
}

/// 持久化失败（磁盘满/权限/路径被占）会上抛给前端：星标回弹 + 错误横幅，
/// 而不是内存假成功、重启后丢收藏（评审发现）。
///
/// async + spawn_blocking 的理由同 `scan_ports`（见本文件顶部）：每次点 ★ 都要
/// 走 merge 读盘 → create_dir_all → write → rename 四次同步文件操作，还可能先阻塞
/// 在与在飞扫描共享的白名单 Mutex 上。压在主线程时，配置目录落在网络卷或磁盘打嗝
/// 就表现为「点一下星标，整个窗口和托盘一起卡住」（评审发现：scan 早已挪走，
/// 这两条一直留在原地）。
#[tauri::command]
pub async fn add_whitelist(key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || whitelist::add(key))
        .await
        .map_err(|e| format!("whitelist task failed: {e}"))?
}

#[tauri::command]
pub async fn remove_whitelist(key: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || whitelist::remove(&key))
        .await
        .map_err(|e| format!("whitelist task failed: {e}"))?
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
    // **尽力全部执行完再报错，不中途 `?` 短路**（评审发现）：`TrayLang` 上面已经
    // 改成新语言了，此处任何一项 set_text 失败若直接返回，后面的菜单项就停在旧
    // 语言 —— 用户看到一份中英混排的托盘菜单，而内部状态已是新语言，再切一次也
    // 修不好（那时相等判断会认为「已经是这个语言了」）。收集错误、跑完全部项，
    // 让下一次切换仍有机会把它们全部对齐。
    let mut failures: Vec<String> = Vec::new();
    let mut note = |r: Result<(), String>| {
        if let Err(e) = r {
            failures.push(e);
        }
    };

    if let Some(items) = app.try_state::<crate::TrayMenuItems>() {
        let (show, quit) = crate::tray_texts(lang);
        note(items.show.set_text(show).map_err(|e| e.to_string()));
        note(
            items
                .settings
                .set_text(crate::settings_text(lang))
                .map_err(|e| e.to_string()),
        );
        note(items.dir.set_lang(lang));
        note(items.quit.set_text(quit).map_err(|e| e.to_string()));
    }
    // macOS 应用菜单的 ⌘Q 替代项 + 设置项 + 「打开目录」菜单（顶部应用菜单栏那份）同步语言
    #[cfg(target_os = "macos")]
    if let Some(items) = app.try_state::<crate::AppMenuItems>() {
        note(
            items
                .quit_to_tray
                .set_text(crate::quit_to_tray_text(lang))
                .map_err(|e| e.to_string()),
        );
        note(
            items
                .settings
                .set_text(crate::settings_text(lang))
                .map_err(|e| e.to_string()),
        );
        note(items.dir.set_lang(lang));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{} menu item(s) failed to re-text: {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}
