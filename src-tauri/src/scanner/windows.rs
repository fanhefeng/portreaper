//! Windows 数据采集与路径规则。
//! 端口：GetExtendedTcpTable（IPv4 + IPv6，无子进程、无 locale 依赖、普通权限可用）。
//! 元数据：sysinfo（长生命周期 System，前端 2s 轮询天然提供 CPU 采样间隔）。
//! 孤儿语义：Windows 不收养孤儿 —— 父 PID 变「悬空」且可能被复用，
//! 因此以「父不存在」+「父创建时间晚于子（槽位复用）」为判定信号。

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Mutex, OnceLock};

use sysinfo::{Pid, ProcessesToUpdate, System, Users};
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
pub(crate) fn normalize_path(p: &str) -> String {
    p.to_lowercase().replace('/', "\\")
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

    // 6. Cargo 产物
    if p.contains("\\target\\debug\\") || p.contains("\\target\\release\\") {
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

pub(crate) fn collect() -> Collected {
    let ports_by_pid = tcp_listeners();

    let mut sys = system().lock().unwrap();
    sys.refresh_processes(ProcessesToUpdate::All, true);
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
        procs.insert(
            pid_u32,
            ProcMeta {
                ppid: proc_.parent().map(|p| p.as_u32()).unwrap_or(0),
                exe_path,
                full_command,
                user,
                start_unix: Some(proc_.start_time()),
                elapsed_secs: proc_.run_time(),
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
            for _ in 0..4 {
                let mut buf = vec![0u32; (size.max(16) as usize).div_ceil(4)];
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
                if ret != NO_ERROR.0 {
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
    }
}
