// 扫描 / 分类 / 终止住在 portreaper-core（无 GUI 依赖的判定引擎）——
// 本 crate 只是它的桌面前端：托盘、窗口生命周期、命令入口、白名单落盘。
mod commands;
mod paths;
mod updater;
mod whitelist;

use std::sync::Mutex;

use tauri::{
    menu::{MenuBuilder, MenuItem, MenuItemBuilder, Submenu, SubmenuBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WindowEvent, Wry,
};

/// 当前界面语言（"zh" / "en"）。唯一读取方是 Windows 的托盘 tooltip
/// （commands.rs update_tray_title）；菜单 re-text 直接用调用参数，不读它。
/// macOS 上只写不读 —— 为跨平台状态形状统一而保留。
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
    pub settings: MenuItem<Wry>,
    pub dir: DirMenuItems,
    pub quit: MenuItem<Wry>,
}

/// macOS 应用菜单里的句柄（语言切换时 re-text）：⌘Q 替代项 + 设置项（⌘,）
/// + 目录菜单的应用菜单栏那份 —— 与托盘双入口。
#[cfg(target_os = "macos")]
pub struct AppMenuItems {
    pub quit_to_tray: MenuItem<Wry>,
    pub settings: MenuItem<Wry>,
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
pub(crate) fn open_app_dir(app: &AppHandle, dir: tauri::Result<std::path::PathBuf>) {
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

/// 「设置…」菜单项文案（托盘 + macOS 应用菜单双入口共用）。
pub(crate) fn settings_text(lang: &str) -> &'static str {
    if lang == "zh" {
        "设置…"
    } else {
        "Settings…"
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

/// 日志系统**自己**没起来时的最后一道线索。
///
/// 这条路径上不能用 `log::` —— 门面还没接上，写进去等于扔掉；而正式版的 `.app`
/// / Windows 无控制台又都吞 stderr，只 eprintln 相当于没报。故直接落一个固定的
/// 临时文件，用户和我们都能按图索骥。
///
/// 四条自我约束：**只 append**、**有大小上限**、**写失败就算了**（`let _`）、
/// **不经过任何日志门面** —— 一个报告日志故障的函数如果自己也可能触发日志，
/// 就会重演 logger.ts 那次自激（46 MB + 打满 CPU）。
///
/// 上限那条是后补的（评审发现）：本函数原本假定「这个文件只在启动失败时才被写到，
/// 故不需要轮转」，而 `install_panic_hook` 把**每一次** panic（含完整 backtrace，
/// 数 KB）都写到这里，这个前提就不成立了。scanner 里一个确定性的解析 panic 会被
/// `spawn_blocking` 兜住、前端每 2 秒重试一次 —— 即约 18 MB/小时、无上限，正是
/// 那次自激的同一形态，只是慢了几个数量级。
const BOOTSTRAP_LOG_MAX_BYTES: u64 = 1024 * 1024;

fn log_bootstrap_failure(msg: &str) {
    eprintln!("{msg}");
    let path =
        std::env::temp_dir().join(format!("portreaper-{}-bootstrap.log", paths::env_label()));
    // 到顶就**停写**，不做截断重建：诊断价值全在最早那几条（第一现场），
    // 尾部无非是同一条 backtrace 的第 N 万份副本。文件落在 temp_dir，由系统回收。
    if std::fs::metadata(&path).is_ok_and(|m| m.len() >= BOOTSTRAP_LOG_MAX_BYTES) {
        return;
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        use std::io::Write;
        let _ = writeln!(f, "[{}] {msg}", env!("CARGO_PKG_VERSION"));
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
        // 1 MiB —— 与上方注释里的单位一致（曾写 1_000_000，是 1 MB 不是 1 MiB）
        .max_file_size(1024 * 1024)
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
/// 全局 panic 钩子 —— 在 Builder 之前装好。
///
/// 没有它的话，release 下的 panic 是**完全静默**的：`main.rs` 是
/// `windows_subsystem = "windows"`（无控制台），macOS 的 `.app` 同样吞掉 stderr。
/// 而 scanner 大量解析 lsof / ps 的文本输出、还有一整片 Windows FFI（那半边没有
/// 真机 QA），托盘线程与 `RunEvent::Ready` 里 spawn 的抢焦点线程中的 panic 会
/// 直接终止进程 —— 用户看到的是「托盘图标凭空消失」，日志里一个字都没有。
///
/// 三条自我约束，全部照抄 `src/logger.ts` 那次 46 MB 自激的教训：
/// - **钩子内部不可能再 panic**：只用 `&str`/`String` 与已就位的 IO 兜底路径；
/// - **两路都写**：`log::error!` 走正常日志（插件未注册时它是空操作，不报错），
///   同时再写一份 `log_bootstrap_failure`（它的定位就是「日志系统靠不住时的兜底」，
///   只追加、带大小上限、写失败就算了 —— 上限正是为本钩子加的，见那边的注释）；
/// - **链回默认钩子**：debug 下仍要有 Rust 原生的 panic 输出与 backtrace。
fn install_panic_hook() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_string());
        // payload 只可能是 &str 或 String（panic! 宏的两种形态），其它类型放弃取值
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string panic payload>".to_string());
        let thread = std::thread::current();
        let msg = format!(
            "PANIC in thread {:?} at {location}: {payload}\n{}",
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture()
        );
        log::error!("{msg}");
        log_bootstrap_failure(&msg);
        default_hook(info);
    }));
}

/// 把主窗口显示出来并抢到最前。三处调用点共用：托盘「显示窗口」、macOS 的
/// `RunEvent::Reopen`、Windows 单实例守卫收到的二次启动。
///
/// 本函数从主线程内联调用即可。**「必须 spawn 一个线程再 set_focus」那条实测约束
/// 只对启动路径成立**，它的正文在 `RunEvent::Ready` 那段 —— 成因是启动/激活序列
/// 随后会把就地执行的激活覆盖掉，而这里的三个调用点（托盘、`Reopen`、Windows
/// 单实例）都发生在启动早就结束之后，没有东西会来覆盖。
///
/// 这句话原本被写成对本函数的普遍约束，与函数体自相矛盾（评审发现）。两个方向的
/// 误读都有代价：照注释把三处都改成 spawn 是凭空多起三个线程；反过来认定注释是
/// 错的、顺手把 `Ready` 那段也「简化」掉，则会复发「启动时窗口压在别人下面」。
fn focus_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// 打开设置弹窗：先把窗口带到前台，再通知 webview（SettingsModal 在前端）。
/// 托盘「设置…」与 macOS 应用菜单 ⌘, 共用（两处菜单项同 id "open-settings"，
/// 事件全局派发，见托盘 on_menu_event 的注释）。事件监听走 capabilities 里
/// 已有的 core:default（含 event:default），白名单零改动。
fn open_settings(app: &AppHandle) {
    focus_main(app);
    if let Err(e) = app.emit_to("main", "open-settings", ()) {
        log::warn!("emit open-settings failed: {e}");
    }
}

pub fn run() {
    // 第一件事，早于 Builder：setup 闭包、托盘线程、抢焦点线程里的 panic
    // 都得被它盖住
    install_panic_hook();

    let builder = tauri::Builder::default();

    // 单实例插件官方要求**排在插件链最前**（它要在其它插件初始化前决定「本进程
    // 该不该活下去」）。macOS 不注册：那边根本起不了第二个进程，见 Cargo.toml。
    #[cfg(windows)]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
        // 第二次启动的进程会立刻退出，这个回调在**已在运行的那个**里执行 ——
        // 用户的意图就是「把它叫出来」
        focus_main(app);
    }));

    let builder = builder
        .plugin(tauri_plugin_opener::init())
        // 应用内更新。纯 Rust 侧注册（不装 npm 包）：检查/安装都走本 crate 的
        // updater.rs 命令，进度经 IPC Channel 回传 —— capabilities 白名单零改动。
        .plugin(tauri_plugin_updater::Builder::new().build())
        // 窗口尺寸/位置记忆。
        //
        // 只记 SIZE | POSITION，**绝不能加 VISIBLE / 用 ALL**：本应用「关窗即隐藏、
        // 只从托盘退出」，恢复一个 hidden 状态会与 `RunEvent::Ready` 里那段实测调优
        // 过的 show + 重试抢焦点直接打架，表现是启动后窗口再也不出现。
        //
        // 文件名按环境分叉，与 `portreaper_core::paths` 的 dev/prod 隔离同向：插件把
        // 状态写进 Tauri 的 `app_config_dir()`，而那个目录**不认识**本项目的 `dev/`
        // 约定 —— 不分文件名的话，`pnpm tauri dev` 会去改正式版的窗口几何。
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::SIZE
                        | tauri_plugin_window_state::StateFlags::POSITION,
                )
                .with_filename(format!(".window-state-{}.json", paths::env_label()))
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            commands::scan_ports,
            commands::kill_process,
            commands::get_platform,
            commands::open_log_dir,
            commands::add_whitelist,
            commands::remove_whitelist,
            commands::update_tray_title,
            commands::set_tray_language,
            commands::set_window_theme,
            updater::check_update,
            updater::install_update,
            updater::restart_app,
            updater::open_release_page,
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
            // 「设置…」按 HIG 惯例挂 ⌘,。菜单栏虽不可见（Accessory），但 key
            // equivalent 仍经这份隐形菜单路由 —— 与 ⌘Q 同一机制。id 与托盘的
            // 设置项相同，点击由托盘 on_menu_event 全局派发统一处理。
            let settings = MenuItemBuilder::with_id("open-settings", settings_text(lang))
                .accelerator("Cmd+,")
                .build(handle)?;
            let app_submenu = SubmenuBuilder::new(handle, "Portreaper")
                .about(None)
                .separator()
                .item(&settings)
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
            handle.manage(AppMenuItems {
                quit_to_tray,
                settings,
                dir,
            });
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
            match paths::log_dir() {
                Ok(dir) => {
                    if let Err(e) = app.handle().plugin(build_log_plugin(dir)) {
                        log_bootstrap_failure(&format!("failed to init logging: {e}"));
                    }
                }
                Err(e) => log_bootstrap_failure(&format!("failed to resolve log dir: {e}")),
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
            // 与 macOS 应用菜单的设置项同 id：事件全局派发，一处 handler 覆盖双入口
            let settings_item =
                MenuItemBuilder::with_id("open-settings", settings_text(lang)).build(app)?;
            // 托盘的 devtools 项挂在根菜单（不入子菜单），故 devtools_in_submenu=false
            let dir = build_dir_menu(&*app, lang, false)?;
            let quit_item = MenuItemBuilder::with_id("quit", quit_text).build(app)?;
            // 用 shadowing 条件插入 devtools 项：prod 下不存在该行，故无 unused_mut 告警。
            let menu = {
                let b = MenuBuilder::new(app)
                    .item(&show_item)
                    .item(&settings_item)
                    .item(&dir.open_dir);
                #[cfg(debug_assertions)]
                let b = b.item(&dir.open_devtools);
                b.separator().item(&quit_item).build()?
            };
            app.manage(TrayLang(Mutex::new(lang)));
            app.manage(TrayMenuItems {
                show: show_item,
                settings: settings_item,
                dir,
                quit: quit_item,
            });
            // 常驻扫描器：必须跨轮询存活，否则 Windows 的 CPU 列恒为 0%
            // （采样区间就是两次 scan 之间的间隔，详见 commands::ScannerState）
            app.manage(commands::ScannerState(Mutex::new(
                portreaper_core::Scanner::new(),
            )));
            // check_update 找到的待装更新，install_update 消费（详见 updater.rs）
            app.manage(updater::PendingUpdate(Mutex::new(None)));

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
                    "show" => focus_main(app),
                    "open-settings" => open_settings(app),
                    "open-config-dir" => open_app_dir(app, paths::config_dir()),
                    "open-data-dir" => open_app_dir(app, paths::data_dir()),
                    "open-cache-dir" => open_app_dir(app, paths::cache_dir()),
                    "open-log-dir" => open_app_dir(app, paths::log_dir()),
                    "open-temp-dir" => open_app_dir(app, paths::temp_dir()),
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
                        // 「显示窗口并抢到最前」只有 focus_main 一处实现 ——
                        // 这里曾内联重写过一遍，是该规则的第四条漏网路径
                        focus_main(tray.app_handle());
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
            match event {
                // 启动时把窗口抢到最前 —— Accessory / LSUIElement 应用不会被
                // macOS 自动激活：窗口 orderFront 了，应用却不是 active app，于是
                // 压在别的窗口底下、也拿不到键盘焦点（用户报「每次启动打开之后没
                // 显示在最前面」）。set_focus 走的 makeKeyAndOrderFront +
                // activateIgnoringOtherApps 正是缺的那一步；此前只有托盘点击 /
                // 托盘「显示」/ Reopen 三条路径调它，启动路径一条都没有。
                //
                // 两个实测约束，别按「看起来更直接」的写法改回去：
                // 1) 必须开线程，不能在本回调里同步调 set_focus —— tao 的实现是
                //    run_on_main，已在主线程时就地执行，那样的 activate 会被尚未
                //    跑完的启动流程盖掉（实测拿不到焦点）；从别的线程发起才会排进
                //    主线程队列、在本回调返回之后执行，实测一次即中。
                // 2) 必须是 Ready 而不是 setup。setup 里还要先 set_activation_policy
                //    降到 Accessory，先降策略后抢焦点，顺序不能反。
                // 重试是给冷启动兜底（实测冷启动时 Ready 本身能晚 ~1.8s）。锁屏时
                // 系统禁止任何应用抢前台，届时重试到上限放弃即可 —— 属预期行为。
                tauri::RunEvent::Ready => {
                    if let Some(w) = app.get_webview_window("main") {
                        let _ = w.show();
                        std::thread::spawn(move || {
                            for _ in 0..8 {
                                let _ = w.set_focus();
                                std::thread::sleep(std::time::Duration::from_millis(200));
                                if w.is_focused().unwrap_or(false) {
                                    break;
                                }
                            }
                        });
                    }
                }
                // 应用已在运行时又被启动一次（Spotlight / Launchpad / Finder）。
                // 无条件前置：窗口「可见但被别的窗口挡住」恰恰是用户再点一次的
                // 原因，按 has_visible_windows 短路会让这次点击毫无反应。
                tauri::RunEvent::Reopen { .. } => focus_main(app),
                _ => {}
            }
            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app, event);
            }
        });
}
