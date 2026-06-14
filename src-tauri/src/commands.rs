use crate::{platform, scanner, whitelist};

/// async + spawn_blocking（评审发现）：Tauri 2 的非 async 命令在主线程执行，
/// scan() 每 2s shell 出 lsof + 两次 ps + launchctl（几十到几百毫秒）会周期性
/// 阻塞事件循环（托盘/窗口事件卡顿）。挪到阻塞线程池，主线程零占用。
#[tauri::command]
pub async fn scan_ports() -> Result<Vec<scanner::ProcessEntry>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let wl = whitelist::get_all();
        scanner::scan(&wl)
    })
    .await
    .map_err(|e| format!("scan task failed: {e}"))
}

/// 终止进程。`start_unix` 是扫描时捕获的创建时间 —— kill 前重新核对，
/// 防止 scan 与点击之间 PID 被复用导致误杀（Windows 复用尤其激进）。
/// async 理由同 scan_ports（macOS 分支 shell 出 ps + kill 两个子进程）。
#[tauri::command]
pub async fn kill_process(pid: u32, force: bool, start_unix: Option<u64>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || platform::kill(pid, force, start_unix))
        .await
        .map_err(|e| format!("kill task failed: {e}"))?
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

#[tauri::command]
pub fn get_whitelist() -> Vec<String> {
    whitelist::get_all()
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
            use tauri::Manager;
            let lang = app
                .try_state::<crate::TrayLang>()
                .map(|l| *l.0.lock().unwrap())
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
    use tauri::Manager;
    let lang: &'static str = if lang.to_lowercase().starts_with("zh") {
        "zh"
    } else {
        "en"
    };
    if let Some(state) = app.try_state::<crate::TrayLang>() {
        *state.0.lock().unwrap() = lang;
    }
    if let Some(items) = app.try_state::<crate::TrayMenuItems>() {
        let (show, quit) = crate::tray_texts(lang);
        let (dir, config, logs) = crate::dir_menu_texts(lang);
        items.show.set_text(show).map_err(|e| e.to_string())?;
        items.open_dir.set_text(dir).map_err(|e| e.to_string())?;
        items
            .open_config
            .set_text(config)
            .map_err(|e| e.to_string())?;
        items.open_logs.set_text(logs).map_err(|e| e.to_string())?;
        items.quit.set_text(quit).map_err(|e| e.to_string())?;
    }
    // macOS 应用菜单的 ⌘Q 替代项（quit-to-tray）同步语言
    #[cfg(target_os = "macos")]
    if let Some(items) = app.try_state::<crate::AppMenuItems>() {
        items
            .quit_to_tray
            .set_text(crate::quit_to_tray_text(lang))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
