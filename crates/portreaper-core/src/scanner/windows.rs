//! Windows 数据采集与路径规则。
//! 端口：GetExtendedTcpTable（IPv4 + IPv6，无子进程、无 locale 依赖、普通权限可用）。
//! 元数据：sysinfo（长生命周期 `System`，由 [`PlatformState`] 持有 —— 相邻两次
//! refresh 之间的间隔**就是** CPU 百分比的采样区间，见 `scanner::CpuSampling`）。
//! 孤儿语义：Windows 不收养孤儿 —— 父 PID 变「悬空」且可能被复用，
//! 因此以「父不存在」+「父创建时间晚于子（槽位复用）」为判定信号。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::OnceLock; // KnownPaths 的一次性探测缓存（System 已改由 PlatformState 持有）

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6ROW_OWNER_PID, MIB_TCP6TABLE_OWNER_PID, MIB_TCPROW_OWNER_PID,
    MIB_TCPTABLE_OWNER_PID, MIB_TCP_STATE_ESTAB, MIB_TCP_STATE_LISTEN, TCP_TABLE_OWNER_PID_ALL,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
    FOLDERID_RoamingAppData, FOLDERID_Windows, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
};

use super::classify::ReasonCode;
use super::identify::{
    basename, is_script_runtime, project_binary_label, script_runtime_label, strip_exe, AppIdentity,
};
use super::model::{Collected, Listener, ParentRef, ProcMeta};
use super::{
    AUTOMATION_CATEGORY, DEV_SCRIPT_CATEGORY, INSTALLED_APP_CATEGORY, SYSTEM_CATEGORY,
    UNKNOWN_CATEGORY, USER_BINARY_CATEGORY,
};

// ---------------------------------------------------------------------------
// 已知文件夹（SHGetKnownFolderPath，比环境变量可靠：位数无关、重定向感知）
// ---------------------------------------------------------------------------

/// 全部小写、以 `\` 结尾的前缀集合。测试可手工构造。
///
/// 私有：平台叶子文件的对外面**只有**那组 cfg 对称函数 + PlatformState —— 这个
/// 类型连同 `paths()` / `identify_app_with` 都是本文件的实现细节（评审发现：
/// 曾标 pub(crate) 却无任何跨模块使用，可见性自身就该把边界说清楚）。
struct KnownPaths {
    pub windows_dir: String,       // c:\windows\
    pub program_files: String,     // c:\program files\
    pub program_files_x86: String, // c:\program files (x86)\
    pub local_appdata: String,     // c:\users\<u>\appdata\local\
    pub roaming_appdata: String,   // c:\users\<u>\appdata\roaming\
    pub program_data: String,      // c:\programdata\
}

impl KnownPaths {
    fn detect() -> Self {
        fn known(id: &windows::core::GUID) -> String {
            unsafe {
                match SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
                    Ok(pw) => {
                        let s = pw.to_string().unwrap_or_default();
                        CoTaskMemFree(Some(pw.0 as *const c_void));
                        normalize_prefix(&s)
                    }
                    Err(_) => String::new(),
                }
            }
        }
        KnownPaths {
            windows_dir: known(&FOLDERID_Windows),
            program_files: known(&FOLDERID_ProgramFiles),
            program_files_x86: known(&FOLDERID_ProgramFilesX86),
            local_appdata: known(&FOLDERID_LocalAppData),
            roaming_appdata: known(&FOLDERID_RoamingAppData),
            program_data: known(&FOLDERID_ProgramData),
        }
    }

    fn windows_apps(&self) -> String {
        format!("{}windowsapps\\", self.program_files)
    }
    fn appdata_programs(&self) -> String {
        format!("{}programs\\", self.local_appdata)
    }
    fn appdata_windows_apps(&self) -> String {
        format!("{}microsoft\\windowsapps\\", self.local_appdata)
    }
}

/// 比较用归一化：小写 + 正斜杠转反斜杠。
fn normalize_path(p: &str) -> String {
    p.to_lowercase().replace('/', "\\")
}

/// 同 normalize_path，但保证尾部 `\`（前缀匹配用：避免 `C:\Foo` 误配 `C:\FooBar`）。
fn normalize_prefix(p: &str) -> String {
    let mut s = normalize_path(p);
    if !s.is_empty() && !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

fn paths() -> &'static KnownPaths {
    static PATHS: OnceLock<KnownPaths> = OnceLock::new();
    PATHS.get_or_init(KnownPaths::detect)
}

// ---------------------------------------------------------------------------
// 路径规则（接受 &KnownPaths 注入以便单测）
// ---------------------------------------------------------------------------

pub(crate) fn is_standard_install_path(exe_path: &str) -> bool {
    is_standard_install_with(paths(), exe_path)
}

fn is_standard_install_with(kp: &KnownPaths, exe_path: &str) -> bool {
    // 读不到 exe（MSIX / 提权进程）→ 保守豁免：宁可漏报也不误杀（评审 E5）
    if exe_path.is_empty() {
        return true;
    }
    let p = normalize_path(exe_path);
    [
        kp.windows_dir.as_str(),
        kp.program_files.as_str(),
        kp.program_files_x86.as_str(),
    ]
    .iter()
    .filter(|s| !s.is_empty())
    .any(|prefix| p.starts_with(prefix))
        || (!kp.local_appdata.is_empty()
            && (p.starts_with(&kp.appdata_programs()) || p.starts_with(&kp.appdata_windows_apps())))
        || (!kp.program_data.is_empty()
            && p.starts_with(&format!("{}microsoft\\", kp.program_data)))
}

/// 「这个 exe 确实装在常规安装位置吗」—— **陈述事实**，供 NonstandardPath 取证。
/// 与 `is_standard_install_path` 的豁免语义严格分开：那个对**空路径**返回 true
///（MSIX / 提权进程读不到 exe 时保守豁免，见上），而「路径读不到」是**未知**，
/// 绝不是「装在标准位置」—— 拿它陈述事实会让 Windows 上每个 exe 不可读的孤儿
/// 都被断言成已正规安装。macOS 侧同名函数排除的是 App Translocation 临时目录，
/// 两边都是「把豁免的偏向剔掉，只留事实」。
pub(crate) fn is_conventional_install_path(exe_path: &str) -> bool {
    is_conventional_install_with(paths(), exe_path)
}

fn is_conventional_install_with(kp: &KnownPaths, exe_path: &str) -> bool {
    !exe_path.is_empty() && is_standard_install_with(kp, exe_path)
}

pub(crate) fn is_brew_service_path(_exe_path: &str) -> bool {
    false
}

const SHELLS: &[&str] = &["cmd", "powershell", "pwsh", "bash", "sh", "nu"];

pub(crate) fn is_shell(exe_path: &str) -> bool {
    let name = strip_exe(basename(exe_path)).to_lowercase();
    SHELLS.contains(&name.as_str())
}

/// 存活的「会话根」：Windows 上几乎所有进程链最终都断在已退出的 userinit/smss，
/// 把 explorer/services 等存活系统根视为链的合法终点，否则每个 cmd 里的 dev server 都会误报。
pub(crate) fn is_live_session_root(exe_path: &str) -> bool {
    let name = strip_exe(basename(exe_path)).to_lowercase();
    matches!(
        name.as_str(),
        "explorer" | "services" | "wininit" | "winlogon" | "svchost" | "taskhostw"
    )
}

pub(crate) fn chain_hits_init(_parent_ppid: u32) -> bool {
    false // Windows 无 init PID；链端点 = 父缺失，在 walk 中处理
}

/// 链走到死根（ppid==0 / 父不在快照）时是否算「链到 init」——**是**。
/// Windows 不收养孤儿：父一退出，PID 就悬空/被复用，「父不在表里」正是这里
/// 唯一的链终止形态（`chain_hits_init` 恒 false，无 PID 1 可依）。
/// 与 macos.rs 的同签名钩子成对（那边为 false）—— 平台语义 100% 收敛在
/// 叶子文件，编排层（chain.rs）不再内嵌 `cfg!(windows)`。
pub(crate) fn dead_root_terminates_chain() -> bool {
    true
}

/// 链回溯的「用户可见 App」终点（Windows：installed-app 即可，
/// 存活系统根 explorer/services 另由 is_live_session_root 处理）。
pub(crate) fn is_chain_stopper(_exe_path: &str, category: &str) -> bool {
    category == INSTALLED_APP_CATEGORY
}

pub(crate) fn synth_chain_root() -> ParentRef {
    ParentRef {
        pid: 0,
        label: "System".to_string(),
        category: SYSTEM_CATEGORY.to_string(),
        exe_path: String::new(),
    }
}

/// 直接孤儿：父缺失 ⇒ parent_exited；父存在但创建时间晚于子 ⇒ PID 槽位复用（真实父已死）。
pub(crate) fn direct_orphan(
    ppid: u32,
    meta: &ProcMeta,
    procs: &HashMap<u32, ProcMeta>,
) -> Option<ReasonCode> {
    if ppid == 0 {
        // PID 0/4（System Idle / System）的子进程属于内核侧，路径豁免兜底
        return Some(ReasonCode::ParentExited);
    }
    match procs.get(&ppid) {
        None => Some(ReasonCode::ParentExited),
        Some(parent) => {
            match (parent.start_unix, meta.start_unix) {
                // +1s 容差避免同秒边界误判
                (Some(ps), Some(cs)) if ps > cs + 1 => Some(ReasonCode::PidSlotReused),
                _ => None,
            }
        }
    }
}

/// Windows 路径阶梯。
pub(crate) fn identify_app(full_command: &str, short_command: &str, exe_path: &str) -> AppIdentity {
    identify_app_with(paths(), full_command, short_command, exe_path)
}

fn identify_app_with(
    kp: &KnownPaths,
    full_command: &str,
    short_command: &str,
    exe_path: &str,
) -> AppIdentity {
    if exe_path.is_empty() {
        // 读不到 exe：保守 unknown，标签用进程名（System / Registry / 提权进程等）
        return AppIdentity {
            label: short_command.to_string(),
            category: UNKNOWN_CATEGORY.to_string(),
        };
    }
    let p = normalize_path(exe_path);

    // 0. 脚本/模块身份优先于一切路径判定 —— 决策树共享在
    //    identify::script_identity_step（双平台逐行同构，曾各写一份且真漂移过）。
    //    Windows 侧注入的差异：标签里的运行时名去掉 .exe（模块标签额外小写）；
    //    脚本自身也在标准路径时归 installed-app（macOS 侧对应 system）。
    if let Some(id) = super::identify::script_identity_step(
        full_command,
        short_command,
        strip_exe(short_command),
        &strip_exe(short_command).to_lowercase(),
        |script| is_standard_install_with(kp, script),
        |script| AppIdentity {
            label: strip_exe(basename(script)).to_string(),
            category: INSTALLED_APP_CATEGORY.to_string(),
        },
    ) {
        return id;
    }

    // 0b. 一次性自动化浏览器实例 —— 身份在命令行，不在路径（与阶梯 0 的脚本身份
    //     对称）。必须先于 Program Files / MSIX 阶梯：chrome.exe / msedge.exe 就装在
    //     Program Files，被归 installed-app 即吃硬豁免、永远漏网（KNOWN-GAPS Gap 1
    //     的 Windows 平行情形；判据 --headless 等开关两平台逐字相同，故实现共享）。
    if super::identify::is_automation_instance(full_command) {
        return AppIdentity {
            label: super::identify::automation_label(exe_path, short_command),
            category: AUTOMATION_CATEGORY.to_string(),
        };
    }

    // 0c. 开发工具下载的浏览器 runtime（Playwright 的 %LOCALAPPDATA%\ms-playwright、
    //     Puppeteer 的 .cache\puppeteer）—— 与 macOS 侧 node_modules 下的 Electron.app
    //     同理：它们是项目的开发期 runtime，不是用户安装的应用。必须先于 5b 的
    //     LOCALAPPDATA→installed-app 阶梯，否则孤儿化的下载浏览器会被整体豁免。
    if super::identify::is_dev_tool_runtime_path(exe_path) {
        return AppIdentity {
            label: project_binary_label(exe_path),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 1. MSIX / Store 应用：去掉发布者哈希与版本，取包名友好形式
    if p.starts_with(&kp.windows_apps()) || p.starts_with(&kp.appdata_windows_apps()) {
        let label = msix_friendly_name(exe_path)
            .unwrap_or_else(|| strip_exe(basename(exe_path)).to_string());
        return AppIdentity {
            label,
            category: INSTALLED_APP_CATEGORY.to_string(),
        };
    }

    // 2. Program Files / LOCALAPPDATA\Programs → 已安装应用（标签 = 根目录下的应用文件夹名）
    for root in [
        kp.program_files.as_str(),
        kp.program_files_x86.as_str(),
        &kp.appdata_programs(),
    ] {
        if !root.is_empty() && p.starts_with(root) {
            let label = first_segment_after(exe_path, &p, root.len())
                .unwrap_or_else(|| strip_exe(basename(exe_path)).to_string());
            return AppIdentity {
                label,
                category: INSTALLED_APP_CATEGORY.to_string(),
            };
        }
    }

    // 3. SystemRoot → 系统组件
    if !kp.windows_dir.is_empty() && p.starts_with(&kp.windows_dir) {
        return AppIdentity {
            label: strip_exe(basename(exe_path)).to_string(),
            category: SYSTEM_CATEGORY.to_string(),
        };
    }

    // 4. 脚本运行时（node.exe / python.exe / ...）
    if is_script_runtime(short_command) {
        return AppIdentity {
            label: script_runtime_label(full_command, strip_exe(short_command)),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 5. 包管理器安装的 CLI：scoop / chocolatey / winget links
    // program_data 空值守卫与本函数其余 known-path 分支一致：空前缀会让
    // starts_with 退化成对裸 "chocolatey\" 的匹配（实际不可达，纯风格统一）。
    if p.contains("\\scoop\\")
        || (!kp.program_data.is_empty()
            && p.starts_with(&format!("{}chocolatey\\", kp.program_data)))
        || p.contains("\\microsoft\\winget\\")
    {
        return AppIdentity {
            label: strip_exe(basename(exe_path)).to_string(),
            category: USER_BINARY_CATEGORY.to_string(),
        };
    }

    // 5b. AppData 根目录下的应用（Squirrel/Electron 布局：Discord、Spotify、
    //     GitHub Desktop……装在 %LOCALAPPDATA%\<App>\ 或 %APPDATA%\<App>\，
    //     由会退出的 Update.exe 引导启动）→ installed-app。
    //     评审确认的误杀风险：这些应用监听 localhost 端口、父进程必然退出，
    //     不豁免就会进清扫名单。排除 Temp（临时解包的可执行不算安装）。
    for root in [kp.local_appdata.as_str(), kp.roaming_appdata.as_str()] {
        if !root.is_empty() && p.starts_with(root) && !p.starts_with(&format!("{root}temp\\")) {
            let label = first_segment_after(exe_path, &p, root.len())
                .unwrap_or_else(|| strip_exe(basename(exe_path)).to_string());
            return AppIdentity {
                label,
                category: INSTALLED_APP_CATEGORY.to_string(),
            };
        }
    }

    // 6. Cargo 产物；go run 的临时编译产物（%TEMP%\go-build*\...）同理 ——
    //    与 macOS 侧共用 identify::is_dev_build_artifact（分隔符/大小写归一），
    //    避免两平台各维护一份片段列表而漂移。
    if super::identify::is_dev_build_artifact(exe_path) {
        return AppIdentity {
            label: project_binary_label(exe_path),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 7. 用户目录下的自定义二进制。类别 user-binary 而非 dev-script ——
    //    「位于用户目录」只说明位置，不构成 dev 证据（dev-script 会把
    //    裸孤儿二进制直升 Confirmed 入清扫）。
    if p.contains("\\users\\") {
        return AppIdentity {
            label: project_binary_label(exe_path),
            category: USER_BINARY_CATEGORY.to_string(),
        };
    }

    // 8. fallback
    AppIdentity {
        label: strip_exe(basename(exe_path)).to_string(),
        category: UNKNOWN_CATEGORY.to_string(),
    }
}

/// `...\WindowsApps\Microsoft.WindowsTerminal_1.18.2_x64__8wekyb3d8bbwe\wt.exe`
/// → "Microsoft.WindowsTerminal" 的尾段 → "WindowsTerminal"（尽力而为）。
fn msix_friendly_name(exe_path: &str) -> Option<String> {
    let segments: Vec<&str> = exe_path.split(['\\', '/']).collect();
    let pkg = segments
        .iter()
        .find(|s| s.contains("__") || s.matches('_').count() >= 2)?;
    let name_part = pkg.split('_').next()?;
    let friendly = name_part.rsplit('.').next().unwrap_or(name_part);
    if friendly.is_empty() {
        None
    } else {
        Some(friendly.to_string())
    }
}

/// 取 root 前缀之后的第一个路径段。优先用原始大小写的 exe_path 截取（显示友好），
/// 但 to_lowercase 对个别非 ASCII 字符不保长 —— 字节长度对不上时退回归一化串，
/// 避免按错误偏移切出乱段（评审发现的非 ASCII 路径脆弱点）。
fn first_segment_after(exe_path: &str, normalized: &str, prefix_len: usize) -> Option<String> {
    let source = if exe_path.len() == normalized.len() {
        exe_path
    } else {
        normalized
    };
    let rest = source.get(prefix_len..)?;
    let seg = rest.split(['\\', '/']).next()?;
    if seg.is_empty() || seg.to_lowercase().ends_with(".exe") {
        None // exe 直接位于根目录下，让调用方退回 basename
    } else {
        Some(seg.to_string())
    }
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

/// 一次扫描会话的平台状态。
///
/// Windows 侧持有 `sysinfo::System`：CPU 百分比是**两次 refresh 之间**的增量，
/// 所以这份状态必须跨扫描存活。此前它是进程级 `OnceLock<Mutex<System>>`，
/// 贴合「常驻 GUI 每 2 秒轮询」的唯一场景；改成显式持有后，短命的 CLI 也能
/// 自己决定要不要付采样的代价（见 `scanner::CpuSampling`）——冷启动只 refresh
/// 一次的话，Windows 上每一行的 CPU 都会是 0%。
pub(crate) struct PlatformState {
    sys: System,
}

impl PlatformState {
    pub(crate) fn new() -> Self {
        Self { sys: System::new() }
    }

    /// 只刷新进程表、不做完整采集：为随后的 `collect()` 建立 CPU 采样基线。
    pub(crate) fn warm_up(&mut self) {
        refresh_processes(&mut self.sys);
    }
}

/// 必须用 `_specifics` + 显式 refresh_kind（评审发现的 Windows 核心失效）：便捷的
/// `refresh_processes(All, true)` 内部固定为 `nothing().with_memory().with_cpu()`
/// `.with_disk_usage().with_exe(OnlyIfNotSet)` —— 不含 cmd/cwd/user。Windows 上
/// 这三项受 refresh_kind 门控并提前 return，导致 `proc_.cmd()` 恒空、`cwd()` 恒 None、
/// `user_id()` 恒 None：full_command 退化为纯 exe（无参数）⇒ extract_script_arg /
/// extract_module_arg 拿不到脚本/模块 ⇒ `node.exe vite.js`、`python.exe -m
/// http.server` 永远走不到 dev-script、被路径阶梯当 installed-app 豁免，
/// CLAUDE.md 的核心检测目标在 Windows 上整体失效；cwd 缺失还让重复检测哑火。
///
/// 只勾选实际读取的字段（cmd/cwd/exe/user/memory/cpu），不用 `everything()`：后者
/// 每 2s 还会为全机进程拉取磁盘 IO 计数器、线程列表、完整 environ 块 —— 全部即取即弃
///（评审发现的浪费）。start_time/run_time/ppid/name 随基础进程信息返回，无需开关。
fn refresh_processes(sys: &mut System) {
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_cwd(UpdateKind::Always)
            .with_exe(UpdateKind::OnlyIfNotSet)
            .with_user(UpdateKind::OnlyIfNotSet)
            .with_memory()
            .with_cpu(),
    );
}

/// 创建时间 / 运行时长的净化：start_time()==0 表示读取失败（句柄受限），
/// 此时两个值都不可信 —— start 置 None（kill 走 fail-closed 的 identity_unknown）。
/// elapsed 不能置 0：那等价于宣称「刚启动」，会让 classify 的宽限期恒命中、把一个
/// exe/cmd 可读但创建时间读不到的孤儿 dev server 永久钉在 Possible、永不入清扫/计数
///（评审发现）。创建时间未知 ≠ 刚启动 —— 置为宽限期阈值（10），既不触发宽限降级、
/// 也不伪造一个荒谬的运行时长。
fn sanitize_times(start: u64, run: u64) -> (Option<u64>, u64) {
    if start > 0 {
        (Some(start), run)
    } else {
        (None, super::classify::GRACE_SECS)
    }
}

impl PlatformState {
    pub(crate) fn collect(&mut self) -> Collected {
        let TcpTables {
            listeners: ports_by_pid,
            mut established_local,
        } = tcp_tables();

        refresh_processes(&mut self.sys);
        let sys = &self.sys;
        let users = Users::new_with_refreshed_list();

        let mut procs: HashMap<u32, ProcMeta> = HashMap::new();
        let mut names: HashMap<u32, String> = HashMap::new();

        for (pid, proc_) in sys.processes() {
            let pid_u32 = pid.as_u32();
            let exe_path = proc_
                .exe()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let cmd_parts: Vec<String> = proc_
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect();
            let full_command = if cmd_parts.is_empty() {
                exe_path.clone()
            } else {
                cmd_parts.join(" ")
            };
            let user = proc_
                .user_id()
                .and_then(|uid| users.get_user_by_id(uid))
                .map(|u| u.name().to_string())
                .unwrap_or_default();

            names.insert(pid_u32, proc_.name().to_string_lossy().into_owned());
            // 句柄受限（受保护/提权）进程 sysinfo 读不到创建时间时返回 0 ——
            // 必须净化（评审发现）：start_unix=Some(0) 会让 kill 身份校验恒以
            // pid_reused 误拒（语义应为缺令牌的 identity_unknown）、
            // elapsed 变成 ~56 年的荒谬时长、并污染 direct_orphan 的槽位复用
            // 比较（start=0 的父或子会伪造时间倒挂）。None 走 fail-closed 语义。
            let (start_unix, elapsed_secs) = sanitize_times(proc_.start_time(), proc_.run_time());
            procs.insert(
                pid_u32,
                ProcMeta {
                    ppid: proc_.parent().map(|p| p.as_u32()).unwrap_or(0),
                    exe_path,
                    full_command,
                    user,
                    start_unix,
                    elapsed_secs,
                    // 与 macOS ps pcpu 同口径：单核百分比，多线程可超 100%
                    cpu_percent: proc_.cpu_usage(),
                    rss_kb: proc_.memory() / 1024, // sysinfo 0.33 memory() 为字节
                    tty: None,
                    state: None,
                    tty_orphaned: false,
                },
            );
        }

        // 仅监听者的 cwd：重复 dev server 检测的证据（MSIX/提权进程读不到时缺席）
        let mut cwds: HashMap<u32, String> = HashMap::new();
        for pid in ports_by_pid.keys() {
            if let Some(p) = sys.process(Pid::from_u32(*pid)).and_then(|p| p.cwd()) {
                cwds.insert(*pid, p.to_string_lossy().to_lowercase());
            }
        }

        let listeners = ports_by_pid
            .into_iter()
            .map(|(pid, mut ports)| {
                ports.sort_unstable();
                ports.dedup();
                Listener {
                    pid,
                    ports,
                    user: procs.get(&pid).map(|m| m.user.clone()).unwrap_or_default(),
                    command: names.get(&pid).cloned().unwrap_or_default(),
                }
            })
            .collect();

        // 存活性证据只对自动化实例有意义（唯一消费者是 automation-instance 的
        // DebuggerAttached 否决）。全表已在手，这里只做一次收窄：普通应用的成百上千条
        // 出站连接留在 Collected 里既无用途，又要跨整次扫描持有（与 macOS 侧
        // 「只查候选 PID」的口径对齐 —— 两平台交给 mod.rs 的是同一种稀疏数据）。
        established_local.retain(|pid, _| {
            procs
                .get(pid)
                .is_some_and(|m| super::identify::is_automation_instance(&m.full_command))
        });
        // 去重，与 macOS 侧 parse_established 的数据形状对齐：调试端口上的每一条入站
        // 连接都是同一个本地端口，10 个客户端会推 10 个 9222。消费方只做 contains
        // 判定、语义不受影响，但两平台交给 mod.rs 的数据必须同形，否则任何未来按
        // 「连接数」做判定的改动都会在两个平台上得出不同结论。放在 retain 之后：
        // 此时只剩极少数自动化 PID，排序去重的成本可忽略。
        for ports in established_local.values_mut() {
            ports.sort_unstable();
            ports.dedup();
        }

        Collected {
            listeners,
            procs,
            launchd_pids: Default::default(),
            cwds,
            established_local_ports: established_local,
        }
    }
}

/// dwLocalPort 的低 16 位按网络字节序存放端口。
fn decode_port(dw_local_port: u32) -> u16 {
    let b = dw_local_port.to_le_bytes();
    u16::from_be_bytes([b[0], b[1]])
}

/// 缓冲字节数下界：空表时 API 只要求 4 字节（裸 dwNumEntries），但解析侧按
/// `*const T` 读表头并用 addr_of! 计算首行偏移 —— 让缓冲无条件覆盖完整的
/// size_of::<T>()（含声明的 table[1] 首行，IPv4 28 字节 / IPv6 60 字节），
/// 这些地址计算就永远在分配范围内，无需逐处论证（评审发现，与下方的对齐
/// 问题同源）。取两族最大值。
const MIN_TABLE_BYTES: usize = {
    let v4 = std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>();
    let v6 = std::mem::size_of::<MIB_TCP6TABLE_OWNER_PID>();
    if v4 > v6 {
        v4
    } else {
        v6
    }
};

/// 表缓冲长度（u32 词数）：报告大小与结构体下界取大，向上取整到 4 字节。
fn table_buf_words(reported: u32) -> usize {
    (reported as usize).max(MIN_TABLE_BYTES).div_ceil(4)
}

/// 一次表查询的两路产物：监听端口，以及**本地端**处于 ESTABLISHED 的端口。
/// 后者是自动化实例的存活性证据（KNOWN-GAPS Gap 1/A2）——「调试端口上有客户端
/// 连着」⇒ 有人正在驱动它，一票否决。
/// 私有：与 `KnownPaths` 同理，属本文件的实现细节（`tcp_tables()` 本身就是私有 fn）。
#[derive(Default)]
struct TcpTables {
    listeners: HashMap<u32, Vec<u16>>,
    established_local: HashMap<u32, Vec<u16>>,
}

/// GetExtendedTcpTable：全状态 TCP 表（含 owning PID），IPv4 与 IPv6 各查一次，
/// 按 dwState 分流成 LISTEN / ESTABLISHED 两路。
///
/// 表类型由 `TCP_TABLE_OWNER_PID_LISTENER` 换成 `TCP_TABLE_OWNER_PID_ALL`：行结构
/// 完全相同（MIB_TCPROW_OWNER_PID 本就带 dwState），**无额外系统调用** ——
/// 与 macOS 侧需要多跑一次 lsof 的取舍不同。
///
/// 代价不为零（别照抄成「零成本」）：ALL 表在跑着数据库 / 大量连接的机器上比
/// LISTENER 表大一两个数量级，这些 ESTABLISHED 行会先落进 `established_local`，
/// 由 `collect()` 在 procs 到手后收窄到自动化实例。收窄之所以只能后置，是因为
/// 判据要读进程的命令行，而进程表此刻还没采。
fn tcp_tables() -> TcpTables {
    let mut out = TcpTables::default();

    unsafe {
        for af in [u32::from(AF_INET.0), u32::from(AF_INET6.0)] {
            let mut size: u32 = 0;
            let _ = GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_ALL, 0);
            // 表可能在两次调用间增长，最多重试几次。
            // 缓冲用 Vec<u32> 而非 Vec<u8>：表结构全员 u32，需要 4 字节对齐 ——
            // Vec<u8> 的对齐由分配器碰巧保证，按语言规则属对齐 UB（评审发现）。
            let mut settled = false;
            for _ in 0..4 {
                let mut buf = vec![0u32; table_buf_words(size)];
                let ret = GetExtendedTcpTable(
                    Some(buf.as_mut_ptr() as *mut c_void),
                    &mut size,
                    false,
                    af,
                    TCP_TABLE_OWNER_PID_ALL,
                    0,
                );
                if ret == ERROR_INSUFFICIENT_BUFFER.0 {
                    continue;
                }
                settled = true;
                if ret != NO_ERROR.0 {
                    // 无真机可调试的平台：失败必须留痕，否则表现为「端口列表
                    // 凭空变空」且无任何线索（评审发现，曾静默吞掉错误码）。
                    // release 是 GUI 子系统、无控制台，全靠 tauri-plugin-log 落盘。
                    log::error!(
                        "GetExtendedTcpTable(af={af}) failed with code {ret}; \
                         port list may be incomplete"
                    );
                    break;
                }
                // 全程裸指针、不构造 &T：引用的 provenance 只覆盖 size_of::<T>()
                // （柔性数组只含声明的 table[1] 首行），从引用派生的行指针读第 2 行
                // 起即越界 —— Stacked/Tree Borrows 语义下的 UB（Miri 可复现），与
                // MIN_TABLE_BYTES 注释是同一问题的另一半。addr_of! 经裸指针解引用
                // 派生，保留 buf 整个分配的 provenance。也因此不能写
                // `(*table).table.as_ptr()`：方法调用会先对 1 行的数组取引用。
                if af == u32::from(AF_INET.0) {
                    let table = buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID;
                    let rows = std::slice::from_raw_parts(
                        std::ptr::addr_of!((*table).table).cast::<MIB_TCPROW_OWNER_PID>(),
                        (*table).dwNumEntries as usize,
                    );
                    for row in rows {
                        out.push_row(row.dwState, row.dwOwningPid, row.dwLocalPort);
                    }
                } else {
                    let table = buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID;
                    let rows = std::slice::from_raw_parts(
                        std::ptr::addr_of!((*table).table).cast::<MIB_TCP6ROW_OWNER_PID>(),
                        (*table).dwNumEntries as usize,
                    );
                    for row in rows {
                        out.push_row(row.dwState, row.dwOwningPid, row.dwLocalPort);
                    }
                }
                break;
            }
            if !settled {
                log::error!(
                    "GetExtendedTcpTable(af={af}) kept reporting ERROR_INSUFFICIENT_BUFFER \
                     after 4 retries; port list may be incomplete"
                );
            }
        }
    }

    out
}

impl TcpTables {
    /// 按 dwState 把一行归入 LISTEN / ESTABLISHED（其余状态 —— TIME_WAIT、
    /// SYN_SENT 等 —— 一律丢弃：既不是在提供服务，也不是稳定的存活证据）。
    /// IPv4/IPv6 两个分支共用，避免同一份分流逻辑写两遍而漂移。
    fn push_row(&mut self, state: u32, pid: u32, dw_local_port: u32) {
        let port = decode_port(dw_local_port);
        let bucket = if state == MIB_TCP_STATE_LISTEN.0 as u32 {
            &mut self.listeners
        } else if state == MIB_TCP_STATE_ESTAB.0 as u32 {
            &mut self.established_local
        } else {
            return;
        };
        bucket.entry(pid).or_default().push(port);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_paths() -> KnownPaths {
        KnownPaths {
            windows_dir: "c:\\windows\\".into(),
            program_files: "c:\\program files\\".into(),
            program_files_x86: "c:\\program files (x86)\\".into(),
            local_appdata: "c:\\users\\fhf\\appdata\\local\\".into(),
            roaming_appdata: "c:\\users\\fhf\\appdata\\roaming\\".into(),
            program_data: "c:\\programdata\\".into(),
        }
    }

    #[test]
    fn port_byte_order() {
        // 5173 = 0x1435，网络序低 16 位为 [0x14, 0x35]
        let dw = u32::from_le_bytes([0x14, 0x35, 0, 0]);
        assert_eq!(decode_port(dw), 5173);
        let dw = u32::from_le_bytes([0x01, 0xBB, 0, 0]); // 443
        assert_eq!(decode_port(dw), 443);
    }

    /// 回归（评审发现的引用有效性 UB）：空表时 API 报告 size=4，但对表结构体
    /// 取 &T 要求缓冲 ≥ size_of::<T>()（IPv4 28B / IPv6 60B）—— 旧下界 max(16)
    /// 不足。任何报告值下缓冲都必须同时覆盖两族结构体与报告大小本身。
    #[test]
    fn table_buffer_never_smaller_than_struct_or_reported() {
        for reported in [0u32, 4, 16, 28, 60, 61, 4096] {
            let bytes = table_buf_words(reported) * 4;
            assert!(
                bytes >= std::mem::size_of::<MIB_TCPTABLE_OWNER_PID>(),
                "reported={reported}: {bytes}B < IPv4 表结构体"
            );
            assert!(
                bytes >= std::mem::size_of::<MIB_TCP6TABLE_OWNER_PID>(),
                "reported={reported}: {bytes}B < IPv6 表结构体"
            );
            assert!(
                bytes >= reported as usize,
                "reported={reported}: 缓冲小于报告大小"
            );
        }
    }

    #[test]
    fn standard_paths_case_insensitive() {
        let kp = fake_paths();
        assert!(is_standard_install_with(
            &kp,
            "C:\\Windows\\System32\\svchost.exe"
        ));
        assert!(is_standard_install_with(
            &kp,
            "C:\\PROGRAM FILES\\App\\app.exe"
        ));
        assert!(is_standard_install_with(
            &kp,
            "C:/Users/fhf/AppData/Local/Programs/Microsoft VS Code/Code.exe"
        ));
        assert!(is_standard_install_with(&kp, "")); // 读不到 exe → 保守豁免
        assert!(!is_standard_install_with(
            &kp,
            "C:\\Users\\fhf\\code\\app\\server.exe"
        ));
    }

    /// 豁免谓词对**空 exe 路径**放行（MSIX / 提权进程读不到时保守豁免），
    /// 而事实谓词必须拒绝：「读不到」是未知，不是「装在标准位置」。评审捕获 ——
    /// 拿豁免谓词陈述事实，Windows 上每个 exe 不可读的孤儿都会被断言成已正规安装，
    /// 悄悄吞掉 NonstandardPath 证据（Windows 无人工 QA，这类偏差只能靠测试兜）。
    #[test]
    fn conventional_path_rejects_the_unreadable_exe_carveout() {
        let kp = fake_paths();
        assert!(
            is_standard_install_with(&kp, ""),
            "豁免侧必须继续保守放行（既有语义不动）"
        );
        assert!(
            !is_conventional_install_with(&kp, ""),
            "事实侧必须说「不知道」而非「标准」"
        );

        // 非空路径上两者必须一致 —— 事实谓词只剔除 carve-out，不另立一套规则
        for p in [
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
            "C:\\Windows\\System32\\svchost.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
            "C:\\Users\\fhf\\code\\app\\server.exe",
        ] {
            assert_eq!(
                is_standard_install_with(&kp, p),
                is_conventional_install_with(&kp, p),
                "{p}"
            );
        }
    }

    #[test]
    fn identify_ladder_windows() {
        let kp = fake_paths();

        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe --type=utility",
            "Code.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
        );
        assert_eq!(label, "Microsoft VS Code");
        assert_eq!(cat, "installed-app");

        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(
            &kp,
            "C:\\Windows\\System32\\svchost.exe -k netsvcs",
            "svchost.exe",
            "C:\\Windows\\System32\\svchost.exe",
        );
        assert_eq!(label, "svchost");
        assert_eq!(cat, "system");

        // 关键：node.exe 装在 Program Files，但脚本在用户空间 → 必须 dev-script
        //（否则 Windows 上孤儿 vite 会被 installed-app 路径豁免吞掉 = 永久漏报）
        let AppIdentity { label, category: cat } = identify_app_with(
            &kp,
            "C:\\Program Files\\nodejs\\node.exe C:\\Users\\fhf\\code\\myapp\\node_modules\\vite\\bin\\vite.js",
            "node.exe",
            "C:\\Program Files\\nodejs\\node.exe",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "myapp · vite.js");

        // scoop 安装的运行时同样走 dev-script
        let AppIdentity { label, category: cat } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\scoop\\apps\\nodejs\\node.exe C:\\Users\\fhf\\code\\myapp\\node_modules\\vite\\bin\\vite.js",
            "node.exe",
            "C:\\Users\\fhf\\scoop\\apps\\nodejs\\node.exe",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "myapp · vite.js");

        let AppIdentity { category: cat, .. } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\code\\mytool\\target\\debug\\mytool.exe",
            "mytool.exe",
            "C:\\Users\\fhf\\code\\mytool\\target\\debug\\mytool.exe",
        );
        assert_eq!(cat, "dev-script");

        // go run 临时编译产物（%TEMP%\go-build*）：与 macOS 对齐归 dev-script
        //（Temp 已被 5b 的 installed-app 例外排除，落到本规则）
        let AppIdentity { category: cat, .. } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\go-build123\\b001\\exe\\server.exe",
            "server.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\go-build123\\b001\\exe\\server.exe",
        );
        assert_eq!(cat, "dev-script");

        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(&kp, "", "System", "");
        assert_eq!(label, "System");
        assert_eq!(cat, "unknown");
    }

    /// KNOWN-GAPS Gap 1 的 Windows 平行情形：chrome.exe / msedge.exe 就装在
    /// Program Files，按路径归 installed-app 即吃硬豁免 —— 与 macOS 同源的漏报。
    /// 判据（--headless 等开关）两平台逐字相同，实现共享在 identify.rs。
    #[test]
    fn headless_automation_identified_by_command_line() {
        let kp = fake_paths();
        const CHROME: &str = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";

        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(
            &kp,
            &format!(
                "{CHROME} --headless=new \
                 --user-data-dir=C:\\Users\\fhf\\AppData\\Local\\Temp\\pptr_profile \
                 --remote-debugging-port=9222"
            ),
            "chrome.exe",
            CHROME,
        );
        assert_eq!(label, "chrome · headless");
        assert_eq!(cat, super::super::AUTOMATION_CATEGORY);

        // 对照（防误杀）：同一个 exe、无自动化开关 ⇒ 用户日常的 Chrome 仍豁免
        let AppIdentity { category: cat, .. } =
            identify_app_with(&kp, CHROME, "chrome.exe", CHROME);
        assert_eq!(cat, "installed-app");

        // 对照（A2 反例）：有头的活跃自动化实例同样必须留在 installed-app
        let AppIdentity { category: cat, .. } = identify_app_with(
            &kp,
            &format!("{CHROME} --remote-debugging-port=9222 --user-data-dir=C:\\Users\\fhf\\AppData\\Local\\Temp\\prof"),
            "chrome.exe",
            CHROME,
        );
        assert_eq!(cat, "installed-app");
    }

    /// Playwright / Puppeteer 下载到 %LOCALAPPDATA% 的浏览器 runtime：必须先于
    /// 5b 的 LOCALAPPDATA→installed-app 阶梯归 dev-script，与 macOS 侧
    /// node_modules 下的 Electron.app 是同一条不变量。
    #[test]
    fn downloaded_browser_runtimes_are_dev_scripts() {
        let kp = fake_paths();
        for exe in [
            "C:\\Users\\fhf\\AppData\\Local\\ms-playwright\\chromium-1148\\chrome-win\\chrome.exe",
            "C:\\Users\\fhf\\.cache\\puppeteer\\chrome\\win64-131\\chrome-win64\\chrome.exe",
            "C:\\Users\\fhf\\code\\app\\node_modules\\electron\\dist\\electron.exe",
        ] {
            let AppIdentity { category: cat, .. } = identify_app_with(&kp, exe, "chrome.exe", exe);
            assert_eq!(cat, "dev-script", "{exe}");
        }
    }

    /// 表分流：LISTEN 与 ESTABLISHED 各入各的桶，其余状态（TIME_WAIT 等）丢弃 ——
    /// 它们既不是在提供服务，也不是稳定的存活证据。
    #[test]
    fn tcp_rows_split_by_state() {
        let mut t = TcpTables::default();
        let port_5173 = u32::from_le_bytes([0x14, 0x35, 0, 0]);
        let port_9222 = u32::from_le_bytes([0x24, 0x06, 0, 0]);
        t.push_row(MIB_TCP_STATE_LISTEN.0 as u32, 100, port_5173);
        t.push_row(MIB_TCP_STATE_ESTAB.0 as u32, 100, port_9222);
        t.push_row(12, 100, port_5173); // MIB_TCP_STATE_TIME_WAIT：丢弃
        assert_eq!(t.listeners.get(&100).unwrap(), &vec![5173]);
        assert_eq!(t.established_local.get(&100).unwrap(), &vec![9222]);
    }

    /// 回归（评审捕获的误杀风险）：Squirrel/Electron 布局的 AppData 应用 ——
    /// 父进程（Update.exe 引导器）必然退出，若不归 installed-app 会进清扫名单。
    #[test]
    fn appdata_apps_are_installed_apps() {
        let kp = fake_paths();

        // Discord：%LOCALAPPDATA%\Discord\app-1.0.x\Discord.exe（监听 RPC 端口）
        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Discord\\app-1.0.9151\\Discord.exe",
            "Discord.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Discord\\app-1.0.9151\\Discord.exe",
        );
        assert_eq!(cat, "installed-app");
        assert_eq!(label, "Discord");

        // Spotify：%APPDATA%\Spotify\Spotify.exe（监听 4380/4381）
        let AppIdentity {
            label,
            category: cat,
        } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Roaming\\Spotify\\Spotify.exe",
            "Spotify.exe",
            "C:\\Users\\fhf\\AppData\\Roaming\\Spotify\\Spotify.exe",
        );
        assert_eq!(cat, "installed-app");
        assert_eq!(label, "Spotify");

        // Temp 下解包的可执行不算安装
        let AppIdentity { category: cat, .. } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\unpacked\\server.exe",
            "server.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\unpacked\\server.exe",
        );
        assert_ne!(cat, "installed-app");
    }

    /// 用户目录裸二进制 = user-binary（位置不构成 dev 证据，
    /// 单独的弱孤儿信号只到 Possible，不入清扫）。
    #[test]
    fn bare_user_binary_not_dev_script() {
        let kp = fake_paths();
        let AppIdentity { category: cat, .. } = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\tools\\myserver.exe --port 8080",
            "myserver.exe",
            "C:\\Users\\fhf\\tools\\myserver.exe",
        );
        assert_eq!(cat, "user-binary");
    }

    #[test]
    fn msix_name_extraction() {
        assert_eq!(
            msix_friendly_name(
                "C:\\Program Files\\WindowsApps\\Microsoft.WindowsTerminal_1.18.2_x64__8wekyb3d8bbwe\\wt.exe"
            ),
            Some("WindowsTerminal".to_string())
        );
    }

    #[test]
    fn live_roots_and_shells() {
        assert!(is_live_session_root("C:\\Windows\\explorer.exe"));
        assert!(is_live_session_root("C:\\Windows\\System32\\services.exe"));
        assert!(!is_live_session_root("C:\\Program Files\\App\\app.exe"));
        assert!(is_shell("C:\\Windows\\System32\\cmd.exe"));
        assert!(is_shell("pwsh.exe"));
        assert!(!is_shell("node.exe"));
    }

    #[test]
    fn orphan_semantics() {
        use super::super::model::ProcMeta;
        fn meta(ppid: u32, start: u64) -> ProcMeta {
            ProcMeta {
                ppid,
                exe_path: "C:\\Users\\x\\a.exe".into(),
                full_command: String::new(),
                user: String::new(),
                start_unix: Some(start),
                elapsed_secs: 100,
                cpu_percent: 0.0,
                rss_kb: 0,
                tty: None,
                state: None,
                tty_orphaned: false,
            }
        }
        let mut procs = HashMap::new();
        procs.insert(10, meta(1, 1000)); // 父，早于子创建
        procs.insert(20, meta(10, 2000)); // 正常子
        procs.insert(30, meta(99, 2000)); // 父 99 不存在
        procs.insert(40, meta(50, 2000)); // 父 50 创建时间晚于子 → 槽位复用
        procs.insert(50, meta(1, 3000));

        assert_eq!(direct_orphan(10, &procs[&20], &procs), None);
        assert_eq!(
            direct_orphan(99, &procs[&30], &procs),
            Some(ReasonCode::ParentExited)
        );
        assert_eq!(
            direct_orphan(50, &procs[&40], &procs),
            Some(ReasonCode::PidSlotReused)
        );

        // 回归（评审发现）：创建时间读取失败（净化为 None）的父/子节点
        // 不得伪造「时间倒挂」的槽位复用信号 —— (Some, None)/(None, Some)
        // 任一侧缺失都应判 None
        let mut unreadable_child = meta(10, 0);
        unreadable_child.start_unix = None;
        procs.insert(60, unreadable_child);
        assert_eq!(direct_orphan(10, &procs[&60], &procs), None);
        let mut unreadable_parent = meta(1, 0);
        unreadable_parent.start_unix = None;
        procs.insert(70, unreadable_parent);
        let child_of_unreadable = meta(70, 2000);
        assert_eq!(direct_orphan(70, &child_of_unreadable, &procs), None);
    }

    /// 回归（评审发现）：句柄受限进程 start_time()==0 时必须净化 ——
    /// 否则 kill 校验恒报 pid_reused（应为 identity_unknown）、
    /// UI 显示 ~56 年运行时长。elapsed 净化为宽限期阈值（非 0）：未知创建时间
    /// 不得被当作「刚启动」而触发宽限降级、令受保护孤儿永不入清扫（评审发现）。
    #[test]
    fn unreadable_start_time_sanitized() {
        let (start, elapsed) = sanitize_times(0, 1_770_000_000);
        assert_eq!(start, None);
        assert!(
            elapsed >= super::super::classify::GRACE_SECS,
            "净化后的 elapsed ({elapsed}) 不得落入宽限期 (< GRACE_SECS)，否则受保护孤儿永不入清扫"
        );
        assert_eq!(
            sanitize_times(1_700_000_000, 3600),
            (Some(1_700_000_000), 3600)
        );
    }
}
