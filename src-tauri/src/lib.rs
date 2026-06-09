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

/// macOS 应用菜单里替代 predefined Quit 的 ⌘Q 项句柄（语言切换时 re-text）。
#[cfg(target_os = "macos")]
pub struct AppMenuItems {
    pub quit_to_tray: MenuItem<Wry>,
}

pub(crate) fn tray_texts(lang: &str) -> (&'static str, &'static str) {
    if lang == "zh" {
        ("显示窗口", "退出 Portreaper")
    } else {
        ("Show Window", "Quit Portreaper")
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn quit_to_tray_text(lang: &str) -> &'static str {
    if lang == "zh" {
        "隐藏到托盘"
    } else {
        "Hide to Tray"
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
    let builder = tauri::Builder::default()
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
        ]);

    // 「仅托盘退出」不变量的真正实现（评审 + 实测推翻了旧的 ExitRequested 拦截）：
    // 默认应用菜单的 predefined Quit（⌘Q）直接调 [NSApp terminate:]，而 tao 0.35
    // 没有实现 applicationShouldTerminate:，terminate 既不可阻止也不会发出
    // ExitRequested —— 实测（quit AppleEvent）进程直接退出，旧拦截分支从未生效。
    // 解法：整体提供自定义应用菜单（默认菜单仅在 Builder::menu 缺席时安装），
    // 把 ⌘Q 绑到自定义 quit-to-tray 项，行为与窗口关闭按钮一致（隐藏到托盘）。
    // Dock 右键退出 / 注销关机走 AppleEvent quit，仍真正退出 —— 系统发起的退出
    // 必须放行，否则应用无法被正常关闭（刻意决策）。
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(|handle| {
            use tauri::menu::SubmenuBuilder;
            let quit_to_tray =
                MenuItemBuilder::with_id("quit-to-tray", quit_to_tray_text(detect_lang()))
                    .accelerator("Cmd+Q")
                    .build(handle)?;
            let app_submenu = SubmenuBuilder::new(handle, "Portreaper")
                .about(None)
                .separator()
                .hide()
                .hide_others()
                .show_all()
                .separator()
                .item(&quit_to_tray)
                .build()?;
            // Edit/Window 子菜单必须保留：webview 的 ⌘C/⌘V/⌘X/⌘A 依赖这些
            // predefined 项的 key equivalent；⌘W 走 close_window → 被
            // on_window_event 拦成隐藏，与产品语义一致。
            let edit_submenu = SubmenuBuilder::new(handle, "Edit")
                .undo()
                .redo()
                .separator()
                .cut()
                .copy()
                .paste()
                .select_all()
                .build()?;
            let window_submenu = SubmenuBuilder::new(handle, "Window")
                .minimize()
                .close_window()
                .build()?;
            let menu = MenuBuilder::new(handle)
                .items(&[&app_submenu, &edit_submenu, &window_submenu])
                .build()?;
            handle.manage(AppMenuItems { quit_to_tray });
            Ok(menu)
        })
        .on_menu_event(|app, event| {
            // 应用菜单 ⌘Q：与窗口关闭按钮同语义 —— 隐藏到托盘，不退出。
            // 注意（评审核实）：tauri 把应用菜单与托盘菜单事件派发到同一个全局
            // 监听列表 —— 本 handler 和 TrayIconBuilder::on_menu_event 都会收到
            // 全部菜单事件，互不干扰靠的是 id 不相交，不是通道分离。给应用菜单
            // 项起 id 绝不能复用 "quit"/"show"：复用 "quit" 会让托盘 handler 对
            // ⌘Q 调 app.exit(0)，悄悄重新引入本项修复消灭的整体退出 bug。
            if event.id.as_ref() == "quit-to-tray" {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
        });

    builder
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
            // 注：这里不拦截 ExitRequested{code: None}。它只在「最后一个窗口被
            // Destroyed」时发出（正常关闭已被 prevent_close 拦下，走不到销毁）——
            // 窗口真被销毁属异常状态（如 webview 崩溃），此时 prevent_exit 只会
            // 留下一个无窗口可恢复的僵尸托盘进程；放行退出才是健壮行为。
            // ⌘Q 的「仅托盘退出」语义由上面的自定义应用菜单实现（terminate: 不经
            // 此事件，拦了也没用 —— 实测验证）。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app, event);
            }
        });
}
