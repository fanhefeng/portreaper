use crate::{platform, scanner, whitelist};

#[tauri::command]
pub fn scan_ports() -> Vec<scanner::ProcessEntry> {
    let wl = whitelist::get_all();
    scanner::scan(&wl)
}

/// 终止进程。`start_unix` 是扫描时捕获的创建时间 —— kill 前重新核对，
/// 防止 scan 与点击之间 PID 被复用导致误杀（Windows 复用尤其激进）。
#[tauri::command]
pub fn kill_process(pid: u32, force: bool, start_unix: Option<u64>) -> Result<(), String> {
    platform::kill(pid, force, start_unix)
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
            let _ = app; // app 仅 Windows 分支读取语言状态
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
        items.show.set_text(show).map_err(|e| e.to_string())?;
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

#[tauri::command]
pub fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;
    if let Some(w) = app.get_webview_window("main") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}
