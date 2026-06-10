//! Windows 数据采集与路径规则。
//! 端口：GetExtendedTcpTable（IPv4 + IPv6，无子进程、无 locale 依赖、普通权限可用）。
//! 元数据：sysinfo（长生命周期 System，前端 2s 轮询天然提供 CPU 采样间隔）。
//! 孤儿语义：Windows 不收养孤儿 —— 父 PID 变「悬空」且可能被复用，
//! 因此以「父不存在」+「父创建时间晚于子（槽位复用）」为判定信号。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind, Users};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    GetExtendedTcpTable, MIB_TCP6TABLE_OWNER_PID, MIB_TCPTABLE_OWNER_PID,
    TCP_TABLE_OWNER_PID_LISTENER,
};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_LocalAppData, FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesX86,
    FOLDERID_RoamingAppData, FOLDERID_Windows, SHGetKnownFolderPath, KF_FLAG_DEFAULT,
};

use super::classify::ReasonCode;
use super::identify::{
    basename, is_script_runtime, project_binary_label, script_runtime_label, strip_exe,
};
use super::model::{Collected, Listener, ParentRef, ProcMeta};

// ---------------------------------------------------------------------------
// 已知文件夹（SHGetKnownFolderPath，比环境变量可靠：位数无关、重定向感知）
// ---------------------------------------------------------------------------

/// 全部小写、以 `\` 结尾的前缀集合。测试可手工构造。
pub(crate) struct KnownPaths {
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

/// 比较用归一化：小写 + 正斜杠转反斜杠 + 保证尾部 `\`。
fn normalize_prefix(p: &str) -> String {
    let mut s = p.to_lowercase().replace('/', "\\");
    if !s.is_empty() && !s.ends_with('\\') {
        s.push('\\');
    }
    s
}

/// 比较用归一化（不加尾斜杠）。
fn normalize_path(p: &str) -> String {
    p.to_lowercase().replace('/', "\\")
}

/// 失败留痕（评审发现）：release 构建是 GUI 子系统（`windows_subsystem = "windows"`，
/// 无控制台），eprintln 写入虚空 —— 而 Windows 恰是无真机 QA、唯一靠用户报障
/// 的平台。stderr 照写（dev 下可见），同时追加到 %TEMP%\portreaper.log；
/// 超过 1 MiB 截断重写，防持续性故障刷满磁盘。
fn log_failure(msg: &str) {
    eprintln!("{msg}");
    let path = std::env::temp_dir().join("portreaper.log");
    let too_big = std::fs::metadata(&path)
        .map(|m| m.len() > 1_000_000)
        .unwrap_or(false);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(!too_big)
        .write(too_big)
        .truncate(too_big)
        .open(&path);
    if let Ok(mut f) = file {
        use std::io::Write;
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{ts}] {msg}");
    }
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

/// 链回溯的「用户可见 App」终点（Windows：installed-app 即可，
/// 存活系统根 explorer/services 另由 is_live_session_root 处理）。
pub(crate) fn is_chain_stopper(_exe_path: &str, category: &str) -> bool {
    category == "installed-app"
}

pub(crate) fn synth_chain_root() -> ParentRef {
    ParentRef {
        pid: 0,
        label: "System".to_string(),
        category: "system".to_string(),
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

/// (label, category) —— Windows 路径阶梯。
pub(crate) fn identify_app(
    full_command: &str,
    short_command: &str,
    exe_path: &str,
) -> (String, String) {
    identify_app_with(paths(), full_command, short_command, exe_path)
}

fn identify_app_with(
    kp: &KnownPaths,
    full_command: &str,
    short_command: &str,
    exe_path: &str,
) -> (String, String) {
    if exe_path.is_empty() {
        // 读不到 exe：保守 unknown，标签用进程名（System / Registry / 提权进程等）
        return (short_command.to_string(), "unknown".to_string());
    }
    let p = normalize_path(exe_path);

    // 0. 带脚本参数的运行时优先 —— node.exe 通常装在 Program Files\nodejs\，
    //    若先按路径归 installed-app 会被豁免，Windows 上的孤儿 vite 将永远漏报。
    //    进程身份是「脚本」：脚本在用户空间 ⇒ dev-script；脚本也在标准路径 ⇒ 随应用归类。
    if is_script_runtime(short_command) {
        if let Some(script) = super::identify::extract_script_arg(full_command) {
            if !is_standard_install_with(kp, script) {
                return (
                    script_runtime_label(full_command, strip_exe(short_command)),
                    "dev-script".to_string(),
                );
            }
            return (
                strip_exe(basename(script)).to_string(),
                "installed-app".to_string(),
            );
        }
        // `-m 模块` 调用：身份是模块（python.exe -m http.server）。必须在
        // Program Files / WindowsApps 阶梯之前判，否则按解释器安装位置归
        // installed-app 被豁免（与 macOS 同源的漏报）。
        if let Some(module) = super::identify::extract_module_arg(full_command) {
            return (
                format!("{} · {}", module, strip_exe(short_command).to_lowercase()),
                "dev-script".to_string(),
            );
        }
    }

    // 1. MSIX / Store 应用：去掉发布者哈希与版本，取包名友好形式
    if p.starts_with(&kp.windows_apps()) || p.starts_with(&kp.appdata_windows_apps()) {
        let label = msix_friendly_name(exe_path)
            .unwrap_or_else(|| strip_exe(basename(exe_path)).to_string());
        return (label, "installed-app".to_string());
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
            return (label, "installed-app".to_string());
        }
    }

    // 3. SystemRoot → 系统组件
    if !kp.windows_dir.is_empty() && p.starts_with(&kp.windows_dir) {
        return (
            strip_exe(basename(exe_path)).to_string(),
            "system".to_string(),
        );
    }

    // 4. 脚本运行时（node.exe / python.exe / ...）
    if is_script_runtime(short_command) {
        return (
            script_runtime_label(full_command, strip_exe(short_command)),
            "dev-script".to_string(),
        );
    }

    // 5. 包管理器安装的 CLI：scoop / chocolatey / winget links
    if p.contains("\\scoop\\")
        || p.starts_with(&format!("{}chocolatey\\", kp.program_data))
        || p.contains("\\microsoft\\winget\\")
    {
        return (
            strip_exe(basename(exe_path)).to_string(),
            "user-binary".to_string(),
        );
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
            return (label, "installed-app".to_string());
        }
    }

    // 6. Cargo 产物；go run 的临时编译产物（%TEMP%\go-build*\...）同理 ——
    //    与 macOS 侧共用 identify::is_dev_build_artifact（分隔符/大小写归一），
    //    避免两平台各维护一份片段列表而漂移。
    if super::identify::is_dev_build_artifact(exe_path) {
        return (project_binary_label(exe_path), "dev-script".to_string());
    }

    // 7. 用户目录下的自定义二进制。类别 user-binary 而非 dev-script ——
    //    「位于用户目录」只说明位置，不构成 dev 证据（dev-script 会把
    //    裸孤儿二进制直升 Confirmed 入清扫）。
    if p.contains("\\users\\") {
        return (project_binary_label(exe_path), "user-binary".to_string());
    }

    // 8. fallback
    (
        strip_exe(basename(exe_path)).to_string(),
        "unknown".to_string(),
    )
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

fn system() -> &'static Mutex<System> {
    static SYSTEM: OnceLock<Mutex<System>> = OnceLock::new();
    SYSTEM.get_or_init(|| Mutex::new(System::new()))
}

/// 创建时间 / 运行时长的净化：start_time()==0 表示读取失败（句柄受限），
/// 此时两个值都不可信 —— start 置 None（kill 走 fail-closed 的 ERR_IDENTITY_UNKNOWN）。
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

pub(crate) fn collect() -> Collected {
    let ports_by_pid = tcp_listeners();

    // 毒化恢复：scan 中途 panic 一次不应让后续每轮轮询永久 panic
    //（前端表现为永远 ERR_SCAN_TIMEOUT）。System 只是缓存，半更新状态可安全续用。
    let mut sys = system()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    // 必须用 _specifics + 显式 refresh_kind（评审发现的 Windows 核心失效）：便捷的
    // refresh_processes(All, true) 内部固定为 nothing().with_memory().with_cpu()
    // .with_disk_usage().with_exe(OnlyIfNotSet) —— 不含 cmd/cwd/user。Windows 上
    // 这三项受 refresh_kind 门控并提前 return，导致 proc_.cmd() 恒空、cwd() 恒 None、
    // user_id() 恒 None：full_command 退化为纯 exe（无参数）⇒ extract_script_arg /
    // extract_module_arg 拿不到脚本/模块 ⇒ `node.exe vite.js`、`python.exe -m
    // http.server` 永远走不到 dev-script、被路径阶梯当 installed-app 豁免，
    // CLAUDE.md 的核心检测目标在 Windows 上整体失效；cwd 缺失还让重复检测哑火。
    //
    // 只勾选实际读取的字段（cmd/cwd/exe/user/memory/cpu），不用 everything()：后者
    // 每 2s 还会为全机进程拉取磁盘 IO 计数器、线程列表、完整 environ 块 —— 全部即取即弃
    // （评审发现的浪费）。start_time/run_time/ppid/name 随基础进程信息返回，无需开关。
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
        // ERR_PID_REUSED 误拒（语义应为缺令牌的 ERR_IDENTITY_UNKNOWN）、
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

    Collected {
        listeners,
        procs,
        launchd_pids: Default::default(),
        cwds,
    }
}

/// dwLocalPort 的低 16 位按网络字节序存放端口。
fn decode_port(dw_local_port: u32) -> u16 {
    let b = dw_local_port.to_le_bytes();
    u16::from_be_bytes([b[0], b[1]])
}

/// 缓冲字节数下界：空表时 API 只要求 4 字节（裸 dwNumEntries），但我们随后
/// 会对整个表结构体取 `&T` —— Rust 引用必须覆盖完整的 size_of::<T>()（含
/// 声明的 table[1] 首行，IPv4 28 字节 / IPv6 60 字节），缓冲小于结构体时
/// 引用一经构造即属 UB（评审发现，与下方的对齐问题同源）。取两族最大值。
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

/// GetExtendedTcpTable：LISTEN 态 TCP 表（含 owning PID），IPv4 与 IPv6 各查一次。
fn tcp_listeners() -> HashMap<u32, Vec<u16>> {
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();

    unsafe {
        for af in [u32::from(AF_INET.0), u32::from(AF_INET6.0)] {
            let mut size: u32 = 0;
            let _ =
                GetExtendedTcpTable(None, &mut size, false, af, TCP_TABLE_OWNER_PID_LISTENER, 0);
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
                    TCP_TABLE_OWNER_PID_LISTENER,
                    0,
                );
                if ret == ERROR_INSUFFICIENT_BUFFER.0 {
                    continue;
                }
                settled = true;
                if ret != NO_ERROR.0 {
                    // 无真机可调试的平台：失败必须留痕，否则表现为「端口列表
                    // 凭空变空」且无任何线索（评审发现，曾静默吞掉错误码）。
                    log_failure(&format!(
                        "GetExtendedTcpTable(af={af}) failed with code {ret}; \
                         port list may be incomplete"
                    ));
                    break;
                }
                if af == u32::from(AF_INET.0) {
                    let table = &*(buf.as_ptr() as *const MIB_TCPTABLE_OWNER_PID);
                    let rows = std::slice::from_raw_parts(
                        table.table.as_ptr(),
                        table.dwNumEntries as usize,
                    );
                    for row in rows {
                        map.entry(row.dwOwningPid)
                            .or_default()
                            .push(decode_port(row.dwLocalPort));
                    }
                } else {
                    let table = &*(buf.as_ptr() as *const MIB_TCP6TABLE_OWNER_PID);
                    let rows = std::slice::from_raw_parts(
                        table.table.as_ptr(),
                        table.dwNumEntries as usize,
                    );
                    for row in rows {
                        map.entry(row.dwOwningPid)
                            .or_default()
                            .push(decode_port(row.dwLocalPort));
                    }
                }
                break;
            }
            if !settled {
                log_failure(&format!(
                    "GetExtendedTcpTable(af={af}) kept reporting ERROR_INSUFFICIENT_BUFFER \
                     after 4 retries; port list may be incomplete"
                ));
            }
        }
    }

    map
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

    #[test]
    fn identify_ladder_windows() {
        let kp = fake_paths();

        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe --type=utility",
            "Code.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Programs\\Microsoft VS Code\\Code.exe",
        );
        assert_eq!(label, "Microsoft VS Code");
        assert_eq!(cat, "installed-app");

        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Windows\\System32\\svchost.exe -k netsvcs",
            "svchost.exe",
            "C:\\Windows\\System32\\svchost.exe",
        );
        assert_eq!(label, "svchost");
        assert_eq!(cat, "system");

        // 关键：node.exe 装在 Program Files，但脚本在用户空间 → 必须 dev-script
        //（否则 Windows 上孤儿 vite 会被 installed-app 路径豁免吞掉 = 永久漏报）
        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Program Files\\nodejs\\node.exe C:\\Users\\fhf\\code\\myapp\\node_modules\\vite\\bin\\vite.js",
            "node.exe",
            "C:\\Program Files\\nodejs\\node.exe",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "myapp · vite.js");

        // scoop 安装的运行时同样走 dev-script
        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\scoop\\apps\\nodejs\\node.exe C:\\Users\\fhf\\code\\myapp\\node_modules\\vite\\bin\\vite.js",
            "node.exe",
            "C:\\Users\\fhf\\scoop\\apps\\nodejs\\node.exe",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "myapp · vite.js");

        let (_, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\code\\mytool\\target\\debug\\mytool.exe",
            "mytool.exe",
            "C:\\Users\\fhf\\code\\mytool\\target\\debug\\mytool.exe",
        );
        assert_eq!(cat, "dev-script");

        // go run 临时编译产物（%TEMP%\go-build*）：与 macOS 对齐归 dev-script
        //（Temp 已被 5b 的 installed-app 例外排除，落到本规则）
        let (_, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\go-build123\\b001\\exe\\server.exe",
            "server.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Temp\\go-build123\\b001\\exe\\server.exe",
        );
        assert_eq!(cat, "dev-script");

        let (label, cat) = identify_app_with(&kp, "", "System", "");
        assert_eq!(label, "System");
        assert_eq!(cat, "unknown");
    }

    /// 回归（评审捕获的误杀风险）：Squirrel/Electron 布局的 AppData 应用 ——
    /// 父进程（Update.exe 引导器）必然退出，若不归 installed-app 会进清扫名单。
    #[test]
    fn appdata_apps_are_installed_apps() {
        let kp = fake_paths();

        // Discord：%LOCALAPPDATA%\Discord\app-1.0.x\Discord.exe（监听 RPC 端口）
        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Local\\Discord\\app-1.0.9151\\Discord.exe",
            "Discord.exe",
            "C:\\Users\\fhf\\AppData\\Local\\Discord\\app-1.0.9151\\Discord.exe",
        );
        assert_eq!(cat, "installed-app");
        assert_eq!(label, "Discord");

        // Spotify：%APPDATA%\Spotify\Spotify.exe（监听 4380/4381）
        let (label, cat) = identify_app_with(
            &kp,
            "C:\\Users\\fhf\\AppData\\Roaming\\Spotify\\Spotify.exe",
            "Spotify.exe",
            "C:\\Users\\fhf\\AppData\\Roaming\\Spotify\\Spotify.exe",
        );
        assert_eq!(cat, "installed-app");
        assert_eq!(label, "Spotify");

        // Temp 下解包的可执行不算安装
        let (_, cat) = identify_app_with(
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
        let (_, cat) = identify_app_with(
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
    /// 否则 kill 校验恒报 ERR_PID_REUSED（应为 ERR_IDENTITY_UNKNOWN）、
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
