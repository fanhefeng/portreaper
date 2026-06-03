use crate::{scanner, whitelist};
use std::process::Command;
use tauri::Manager;

#[tauri::command]
pub fn scan_ports() -> Vec<scanner::ProcessEntry> {
    let wl = whitelist::get_all();
    scanner::scan(&wl)
}

#[tauri::command]
pub fn kill_process(pid: u32, force: bool) -> Result<(), String> {
    let signal = if force { "-9" } else { "-15" };
    let output = Command::new("kill")
        .args([signal, &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(())
}

#[tauri::command]
pub fn get_whitelist() -> Vec<String> {
    whitelist::get_all()
}

#[tauri::command]
pub fn add_whitelist(key: String) {
    whitelist::add(key);
}

#[tauri::command]
pub fn remove_whitelist(key: String) {
    whitelist::remove(&key);
}

#[tauri::command]
pub fn update_tray_title(
    app: tauri::AppHandle,
    count: u32,
    suspect_count: u32,
) -> Result<(), String> {
    if let Some(tray) = app.tray_by_id("main-tray") {
        let title = if suspect_count > 0 {
            format!("{} ⚠", count)
        } else {
            format!("{}", count)
        };
        tray.set_title(Some(title.as_str())).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[tauri::command]
pub fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        w.show().map_err(|e| e.to_string())?;
        w.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}
