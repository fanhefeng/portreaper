use serde::Serialize;

use super::classify::{Confidence, ReasonCode};

/// 父进程链上的一个节点（前端 LauncherChain 渲染用）。
#[derive(Debug, Serialize, Clone)]
pub struct ParentRef {
    pub pid: u32,
    pub label: String,
    pub category: String,
    pub exe_path: String,
}

/// 扫描结果的一行 —— 前端表格的数据契约（serde 输出，字段名即 JSON key）。
#[derive(Debug, Serialize, Clone)]
pub struct ProcessEntry {
    pub pid: u32,
    pub ppid: u32,
    pub ports: Vec<u16>, // 一个进程可同时监听多个端口
    pub command: String,
    pub full_command: String,
    pub exe_path: String,
    pub app_label: String,
    pub app_category: String, // "installed-app" | "system" | "dev-script" | "user-binary" | "unknown"
    pub parent_chain: Vec<ParentRef>,
    pub launcher_label: String,
    pub user: String,
    pub tty: String,             // Windows 上恒为空串
    pub elapsed_secs: u64,       // 双平台统一数值秒；前端 formatDuration 渲染
    pub start_unix: Option<u64>, // 进程创建时间（epoch 秒）——kill 时回传做身份校验，防 PID 复用
    pub cpu_percent: f32,
    pub mem_mb: f32,
    pub state: String, // Windows 上恒为空串（无 defunct 概念）
    pub is_zombie_suspect: bool,
    pub confidence: Confidence, // "none" | "possible" | "likely" | "confirmed"
    pub zombie_reasons: Vec<ReasonCode>, // 机器码，前端 i18n 翻译
    pub is_whitelisted: bool,
}

/// 平台 provider 产出的「监听者」：lsof / GetExtendedTcpTable 的归一化结果。
pub(crate) struct Listener {
    pub pid: u32,
    pub ports: Vec<u16>,
    pub user: String,
    /// 监听端口工具看到的短命令名（lsof 的 c 字段 / sysinfo 的 name()）
    pub command: String,
}

/// 平台 provider 产出的每进程元数据（全进程表，供父链回溯）。
pub(crate) struct ProcMeta {
    pub ppid: u32,
    /// 可执行文件完整路径（macOS 来自 ps comm，含空格也准确；Windows 来自 sysinfo exe()）
    pub exe_path: String,
    /// 完整命令行（exe + 参数，尽力而为；可能因权限为空）
    pub full_command: String,
    pub user: String,
    pub start_unix: Option<u64>,
    pub elapsed_secs: u64,
    pub cpu_percent: f32,
    pub rss_kb: u64,
    pub tty: Option<String>,   // Windows: None
    pub state: Option<String>, // Windows: None
    /// macOS 派生信号：有真实 ttysNNN 但该 tty 已无会话首进程（终端死了）
    pub tty_orphaned: bool,
}

/// 一次平台采集的全部产物。
pub(crate) struct Collected {
    pub listeners: Vec<Listener>,
    pub procs: std::collections::HashMap<u32, ProcMeta>,
    /// launchctl 认领的 PID 集合（Windows 恒为空）
    pub launchd_pids: std::collections::HashSet<u32>,
}

/// 喂给纯分类器的进程信号快照 —— 不含任何平台/子进程依赖，可直接构造做表驱动单测。
#[derive(Debug, Default, Clone)]
pub(crate) struct ProcessSnapshot {
    /// ps state（含 'Z' 即 defunct）；Windows None
    pub state: Option<String>,
    pub elapsed_secs: u64,
    /// 直接孤儿信号及其平台语义（macOS: ppid1_orphan；Windows: parent_exited / pid_slot_reused）
    pub direct_orphan: Option<ReasonCode>,
    /// 父链走到 init/死亡根，途中没有任何 installed-app 或存活的系统根（explorer 等）
    pub chain_terminates_at_init: bool,
    /// 父链上存在「自己已被收养（ppid=1）/父已死」的 shell —— 死掉的终端会话
    pub chain_has_orphan_shell: bool,
    /// launchctl 认领（macOS）—— 硬豁免
    pub launchd_managed: bool,
    /// exe 位于 Homebrew 服务路径（/opt/homebrew/opt|Cellar、/usr/local/opt|Cellar）—— 兜底豁免
    pub brew_service_path: bool,
    /// 祖先是 pm2 God Daemon 或自身是 pm2 容器 —— 用户有意托管
    pub pm2_managed: bool,
    /// 有真实 ttysNNN 但 tty 的会话首进程已不在
    pub tty_orphaned: bool,
    /// 标准安装路径 或 类别为 installed-app/system —— 永不自动标记的不变量
    pub exe_is_standard_install: bool,
    /// 命令行命中 dev-server 关键字
    pub dev_keyword: bool,
    /// identify_app 类别为 dev-script
    pub dev_category: bool,
}
