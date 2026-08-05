// 扫描 / 分类 / 终止住在 portreaper-core（无 GUI 依赖的判定引擎）——
// 本 crate 只是它的桌面前端：托盘、窗口生命周期、命令入口、白名单落盘。
mod commands;
mod paths;
mod whitelist;

use std::sync::Mutex;

use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent, Wry,
};

/// 当前界面语言（"zh" / "en"），托盘 tooltip 与菜单共用；
/// 由系统 locale 初始化，前端切换语言时通过 set_tray_language 同步。
pub struct TrayLang(pub Mutex<&'static str>);

/// 「打开目录」子菜单及其项的句柄。托盘与 macOS 应用菜单是双入口，各持一份
/// 本结构（菜单项 id 相同、事件全局派发）——构建走 `build_dir_menu`、语言
/// 切换走 `set_lang`，新增目录项只需改这两处，四个手写同步点就此消失
/// （评审发现：此前构建 ×2 + re-text ×2 逐字重复约 80 行，漏改即静默 bug）。
pub struct DirMenuItems {
    pub open_dir: Submenu<Wry>,
    pub open_config: MenuItem<Wry>,
    pub open_data: MenuItem<Wry>,
    pub open_cache: MenuItem<Wry>,
    pub open_logs: MenuItem<Wry>,
    pub open_temp: MenuItem<Wry>,
    /// 「打开调试控制台」—— 仅 dev 构建注册（DA-3）。
    #[cfg(debug_assertions)]
    pub open_devtools: MenuItem<Wry>,
}

impl DirMenuItems {
    /// 子菜单标题 + 全部目录项（含 devtools）一次 re-text。
    pub(crate) fn set_lang(&self, lang: &'static str) -> Result<(), String> {
        let dt = dir_menu_texts(lang);
        let e = |e: tauri::Error| e.to_string();
        self.open_dir.set_text(dt.title).map_err(e)?;
        self.open_config.set_text(dt.config).map_err(e)?;
        self.open_data.set_text(dt.data).map_err(e)?;
        self.open_cache.set_text(dt.cache).map_err(e)?;
        self.open_logs.set_text(dt.logs).map_err(e)?;
        self.open_temp.set_text(dt.temp).map_err(e)?;
        #[cfg(debug_assertions)]
        self.open_devtools
            .set_text(devtools_text(lang))
            .map_err(e)?;
        Ok(())
    }
}

/// 构建「打开目录」子菜单。devtools 项两个入口的挂载位置不同 —— 应用菜单
/// 挂在本子菜单尾部、托盘挂在托盘根菜单，故由 `devtools_in_submenu` 区分；
/// 句柄始终存入返回值供 re-text。
fn build_dir_menu<M: Manager<Wry>>(
    m: &M,
    lang: &'static str,
    devtools_in_submenu: bool,
) -> tauri::Result<DirMenuItems> {
    let dt = dir_menu_texts(lang);
    let open_config = MenuItemBuilder::with_id("open-config-dir", dt.config).build(m)?;
    let open_data = MenuItemBuilder::with_id("open-data-dir", dt.data).build(m)?;
    let open_cache = MenuItemBuilder::with_id("open-cache-dir", dt.cache).build(m)?;
    let open_logs = MenuItemBuilder::with_id("open-log-dir", dt.logs).build(m)?;
    let open_temp = MenuItemBuilder::with_id("open-temp-dir", dt.temp).build(m)?;
    #[cfg(debug_assertions)]
    let open_devtools = MenuItemBuilder::with_id("open-devtools", devtools_text(lang)).build(m)?;
    let b = SubmenuBuilder::new(m, dt.title)
        .item(&open_config)
        .item(&open_data)
        .item(&open_cache)
        .item(&open_logs)
        .item(&open_temp);
    #[cfg(debug_assertions)]
    let b = if devtools_in_submenu {
        b.separator().item(&open_devtools)
    } else {
        b
    };
    #[cfg(not(debug_assertions))]
    let _ = devtools_in_submenu;
    let open_dir = b.build()?;
    Ok(DirMenuItems {
        open_dir,
        open_config,
        open_data,
        open_cache,
        open_logs,
        open_temp,
        #[cfg(debug_assertions)]
        open_devtools,
    })
}

/// 托盘菜单项句柄 —— 语言切换时直接 set_text，无需重建菜单。
pub struct TrayMenuItems {
    pub show: MenuItem<Wry>,
    pub dir: DirMenuItems,
    pub quit: MenuItem<Wry>,
}

/// macOS 应用菜单里的句柄（语言切换时 re-text）：⌘Q 替代项 + 目录菜单的
/// 应用菜单栏那份 —— 与托盘双入口。
#[cfg(target_os = "macos")]
pub struct AppMenuItems {
    pub quit_to_tray: MenuItem<Wry>,
    pub dir: DirMenuItems,
}

pub(crate) fn tray_texts(lang: &str) -> (&'static str, &'static str) {
    if lang == "zh" {
        ("显示窗口", "退出 Portreaper")
    } else {
        ("Show Window", "Quit Portreaper")
    }
}

/// 「打开目录」子菜单的全部文案（具名以免多项 tuple 错位）。
pub(crate) struct DirMenuTexts {
    pub title: &'static str,
    pub config: &'static str,
    pub data: &'static str,
    pub cache: &'static str,
    pub logs: &'static str,
    pub temp: &'static str,
}

pub(crate) fn dir_menu_texts(lang: &str) -> DirMenuTexts {
    if lang == "zh" {
        DirMenuTexts {
            title: "打开目录",
            config: "配置目录",
            data: "数据目录",
            cache: "缓存目录",
            logs: "日志目录",
            temp: "临时目录",
        }
    } else {
        DirMenuTexts {
            title: "Open Folder",
            config: "Config Folder",
            data: "Data Folder",
            cache: "Cache Folder",
            logs: "Logs Folder",
            temp: "Temp Folder",
        }
    }
}

/// 在系统文件管理器里打开 app 自建目录。目录可能尚未创建（config 仅在首次保存
/// 白名单时落盘），故先 create_dir_all 兜底，避免打开一个不存在的路径而失败。
/// 失败只记日志、不打断用户（菜单点击没有 UI 反馈通道）。
fn open_app_dir(app: &AppHandle, dir: tauri::Result<std::path::PathBuf>) {
    use tauri_plugin_opener::OpenerExt;
    let path = match dir {
        Ok(p) => p,
        Err(e) => {
            log::warn!("resolve app dir failed: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::create_dir_all(&path) {
        log::warn!("create dir {} failed: {e}", path.display());
    }
    if let Err(e) = app
        .opener()
        .open_path(path.to_string_lossy().into_owned(), None::<&str>)
    {
        log::warn!("open dir {} failed: {e}", path.display());
    }
}

/// 「打开调试控制台」菜单项文案 —— 仅 dev 构建存在（DA-3：调试入口不进 prod）。
#[cfg(debug_assertions)]
pub(crate) fn devtools_text(lang: &str) -> &'static str {
    if lang == "zh" {
        "打开调试控制台"
    } else {
        "Open DevTools"
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

/// 分环境日志插件。debug：stdout + 文件、Debug 级（dev 终端能看到，便于调试）；
/// release：仅文件、Info 级 —— GUI 子系统（main.rs 的 windows_subsystem="windows"）
/// 无控制台，macOS 的 `.app` 同样吞掉 stdout，故正式版只靠落盘才有故障线索。
/// 文件按 1 MiB 轮转且只留一份，防持续性故障刷满磁盘（沿用旧 windows log_failure 阈值）。
fn build_log_plugin<R: tauri::Runtime>(
    log_dir: std::path::PathBuf,
) -> tauri::plugin::TauriPlugin<R> {
    use tauri_plugin_log::{Builder, RotationStrategy, Target, TargetKind};

    let level = if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    };

    let mut builder = Builder::new()
        .level(level)
        .max_file_size(1_000_000)
        .rotation_strategy(RotationStrategy::KeepOne)
        .target(Target::new(TargetKind::Folder {
            path: log_dir,
            file_name: Some(paths::log_file_name()),
        }));

    if cfg!(debug_assertions) {
        builder = builder.target(Target::new(TargetKind::Stdout));
    }

    builder.build()
}

/// 桌面端唯一入口（`main.rs` 只是它的薄壳）。
///
/// 刻意**没有** `#[cfg_attr(mobile, tauri::mobile_entry_point)]`：那是移动端脚手架
/// 残留，与 `Cargo.toml` 里已删掉的 staticlib/cdylib 是同一批。桌面构建下 `mobile`
/// cfg 永不成立，该属性从未展开过 —— 无移动端计划，留着只会让人以为存在移动端支持。
pub fn run() {
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::scan_ports,
            commands::kill_process,
            commands::get_platform,
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
    // 应用现已是 Accessory 策略（见 setup，无 Dock 图标）：这份菜单在菜单栏里
    // 不可见，但 key equivalent 仍经它路由 —— ⌘C/⌘V/⌘W/⌘Q 都依赖它，不能删。
    // 注销关机走 AppleEvent quit，仍真正退出 —— 系统发起的退出必须放行，
    // 否则应用无法被正常关闭（刻意决策）。
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(|handle| {
            use tauri::menu::SubmenuBuilder;
            let lang = detect_lang();
            let quit_to_tray = MenuItemBuilder::with_id("quit-to-tray", quit_to_tray_text(lang))
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
            // 「打开目录」菜单 —— 与托盘子菜单复用相同 id：点击事件由托盘的
            // on_menu_event 全局派发处理（tauri 把应用菜单与托盘菜单事件发到
            // 同一监听列表），故此处只建项 + 存句柄供语言切换，无需新增 handler。
            let dir = build_dir_menu(handle, lang, true)?;
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
                .items(&[&app_submenu, &dir.open_dir, &edit_submenu, &window_submenu])
                .build()?;
            handle.manage(AppMenuItems { quit_to_tray, dir });
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
            // 日志系统运行期注册：此时才拿得到 AppHandle 以解析分环境目录。
            // 放在最前面，让白名单损坏等早期告警也能落盘。
            match paths::log_dir(app.handle()) {
                Ok(dir) => {
                    if let Err(e) = app.handle().plugin(build_log_plugin(dir)) {
                        eprintln!("failed to init logging: {e}");
                    }
                }
                Err(e) => eprintln!("failed to resolve log dir: {e}"),
            }
            log::info!(
                "Portreaper {} starting (env={})",
                env!("CARGO_PKG_VERSION"),
                paths::env_label()
            );

            // 引擎自解析的目录必须与 Tauri 的解析一致，否则 GUI 与 CLI/Raycast
            // 会各写各的白名单。紧跟在日志初始化之后 —— 这条告警必须能落盘。
            paths::assert_matches_tauri(app.handle());

            // Accessory 激活策略：无 Dock 图标、不进 ⌘Tab —— 身份与「常驻托盘」的
            // 产品定位对齐。此前用默认 Regular 策略（有 Dock 图标）却把 ⌘Q 劫持成
            // 隐藏，普通 App 的外观配菜单栏工具的行为，用户会按 HIG 预期 ⌘Q 退出
            // 而感到意外；Accessory 下菜单栏归前台 Regular App 所有、本应用菜单不
            // 可见，「⌘Q 应退出」的预期自然消失。注意：应用菜单仍必须构建
            // （Builder::menu）—— AppKit 对未被响应链消费的按键仍会查询本应用
            // mainMenu 的 key equivalent，webview 的 ⌘C/⌘V/⌘W/⌘Q 全靠这份隐形
            // 菜单路由。副作用：Dock 右键退出的路径随图标一起消失，真正退出只剩
            // 托盘菜单与注销/关机（AppleEvent quit，依旧放行）。
            // 双机制：正式包同时经 src-tauri/Info.plist 的 LSUIElement 在注册期
            // 声明（否则冷启动 Dock 闪现图标 1~2 秒，v0.7.0 实测）；本运行期调用
            // 覆盖 `tauri dev` 的裸二进制（不读 bundle Info.plist），两者都不能删。
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // 白名单路径由引擎决定（CLI / Raycast 读的是同一个函数）——
            // 桌面版绝不自己拼一份，那正是两边分家的起点。
            match portreaper_core::paths::whitelist_path() {
                Some(path) => whitelist::init(path),
                None => log::error!("could not resolve whitelist path; 收藏将无法持久化"),
            }

            let lang = detect_lang();
            let (show_text, quit_text) = tray_texts(lang);
            let show_item = MenuItemBuilder::with_id("show", show_text).build(app)?;
            // 托盘的 devtools 项挂在根菜单（不入子菜单），故 devtools_in_submenu=false
            let dir = build_dir_menu(&*app, lang, false)?;
            let quit_item = MenuItemBuilder::with_id("quit", quit_text).build(app)?;
            // 用 shadowing 条件插入 devtools 项：prod 下不存在该行，故无 unused_mut 告警。
            let menu = {
                let b = MenuBuilder::new(app).item(&show_item).item(&dir.open_dir);
                #[cfg(debug_assertions)]
                let b = b.item(&dir.open_devtools);
                b.separator().item(&quit_item).build()?
            };
            app.manage(TrayLang(Mutex::new(lang)));
            app.manage(TrayMenuItems {
                show: show_item,
                dir,
                quit: quit_item,
            });
            // 常驻扫描器：必须跨轮询存活，否则 Windows 的 CPU 列恒为 0%
            // （采样区间就是两次 scan 之间的间隔，详见 commands::ScannerState）
            app.manage(commands::ScannerState(Mutex::new(
                portreaper_core::Scanner::new(),
            )));

            // 托盘图标：macOS 用专用单色 template 图（纯黑+透明，系统按菜单栏明暗自动反色）。
            // 复用彩色应用图标会被 icon_as_template 压成糊在一起的剪影，故单独嵌入 tray.png；
            // 用 include_bytes! 编译期焊进二进制，避免打包后运行期路径解析失败。
            // 其它平台（Windows）无 template 概念，沿用彩色应用图标。
            #[cfg(target_os = "macos")]
            let icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?;
            #[cfg(not(target_os = "macos"))]
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
                    "open-config-dir" => open_app_dir(app, paths::config_dir(app)),
                    "open-data-dir" => open_app_dir(app, paths::data_dir(app)),
                    "open-cache-dir" => open_app_dir(app, paths::cache_dir(app)),
                    "open-log-dir" => open_app_dir(app, paths::log_dir(app)),
                    "open-temp-dir" => open_app_dir(app, paths::temp_dir(app)),
                    #[cfg(debug_assertions)]
                    "open-devtools" => {
                        if let Some(w) = app.get_webview_window("main") {
                            w.open_devtools();
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
