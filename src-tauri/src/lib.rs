mod commands;
mod platform;
mod scanner;
mod whitelist;

use std::sync::Mutex;

use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager, WindowEvent, Wry,
};

/// 当前界面语言（"zh" / "en"），托盘 tooltip 与菜单共用；
/// 由系统 locale 初始化，前端切换语言时通过 set_tray_language 同步。
pub struct TrayLang(pub Mutex<&'static str>);

/// 托盘菜单项句柄 —— 语言切换时直接 set_text，无需重建菜单。
pub struct TrayMenuItems {
    pub show: MenuItem<Wry>,
    pub quit: MenuItem<Wry>,
}

pub(crate) fn tray_texts(lang: &str) -> (&'static str, &'static str) {
    if lang == "zh" {
        ("显示窗口", "退出 Portreaper")
    } else {
        ("Show Window", "Quit Portreaper")
    }
}

fn detect_lang() -> &'static str {
    let locale = sys_locale::get_locale().unwrap_or_default();
    if locale.to_lowercase().starts_with("zh") {
        "zh"
    } else {
        "en"
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::scan_ports,
            commands::kill_process,
            commands::get_platform,
            commands::get_whitelist,
            commands::add_whitelist,
            commands::remove_whitelist,
            commands::update_tray_title,
            commands::set_tray_language,
            commands::show_main_window,
        ])
        .setup(|app| {
            if let Ok(dir) = app.path().app_config_dir() {
                whitelist::init(dir.join("whitelist.json"));
            }

            let lang = detect_lang();
            let (show_text, quit_text) = tray_texts(lang);
            let show_item = MenuItemBuilder::with_id("show", show_text).build(app)?;
            let quit_item = MenuItemBuilder::with_id("quit", quit_text).build(app)?;
            let menu = MenuBuilder::new(app)
                .items(&[&show_item, &quit_item])
                .build()?;
            app.manage(TrayLang(Mutex::new(lang)));
            app.manage(TrayMenuItems {
                show: show_item,
                quit: quit_item,
            });

            let icon = app
                .default_window_icon()
                .cloned()
                .ok_or("missing default window icon")?;

            let tray_builder = TrayIconBuilder::with_id("main-tray")
                .icon(icon)
                .menu(&menu)
                .show_menu_on_left_click(false);

            // 模板图标 + 菜单栏标题是 macOS 概念；Windows 用彩色图标 + tooltip（见 update_tray_title）
            #[cfg(target_os = "macos")]
            let tray_builder = tray_builder.icon_as_template(true).title("…");

            let _tray = tray_builder
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            match event {
                tauri::RunEvent::Reopen {
                    has_visible_windows,
                    ..
                } => {
                    if !has_visible_windows {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                }
                // 「仅托盘退出」不变量（CLAUDE.md / TESTING-WINDOWS.md 验收项）：
                // 托盘菜单 Quit 走 app.exit(0)，携带 code=Some(0) ⇒ 放行真正退出；
                // ⌘Q / App 菜单 Quit 触发的 ExitRequested code=None ⇒ 改为隐藏到
                // 托盘并 prevent_exit，与窗口关闭按钮行为一致（评审发现：此前 ⌘Q
                // 会绕过托盘直接整体退出，扫描与计数全部消失）。
                tauri::RunEvent::ExitRequested { code, api, .. } if code.is_none() => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.hide();
                    }
                    api.prevent_exit();
                }
                _ => {}
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app, event);
            }
        });
}
