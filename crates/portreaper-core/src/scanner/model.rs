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
    /// 自身 + 全部后代进程的 CPU 合计（纯内存聚合，无额外系统调用）。
    /// **仅展示用，不进 ProcessSnapshot、不参与判定** —— 判定语义是「无人认领」
    /// 而非「费电」，健康的 vite build / tsc 一样能吃满核。
    /// 存在意义（KNOWN-GAPS Gap 1/B）：headless 浏览器把 CPU 全烧在
    /// `--type=gpu-process` 子进程里，被列出的主进程行显示 ~0% ——
    /// 只看行内 CPU 会完整错过一棵满核空转 7 小时的进程树。
    pub cpu_percent_tree: f32,
    pub mem_mb: f32,
    pub state: String, // Windows 上恒为空串（无 defunct 概念）
    pub is_zombie_suspect: bool,
    pub confidence: Confidence, // "none" | "possible" | "likely" | "confirmed"
    pub zombie_reasons: Vec<ReasonCode>, // 机器码，前端 i18n 翻译
    pub is_whitelisted: bool,
    /// 这一行在白名单文件里的键 —— **由引擎产出，前端不要自己再推一遍**。
    ///
    /// 推导规则有个反直觉的分支（`exe_path` 仅在含路径分隔符时可用，否则回退
    /// 全命令行），每多一个前端自行实现，就多一次「在 Raycast 里加的星标桌面版
    /// 认不出来」的机会。`src/model.ts` 的 `whitelistKey()` 因历史原因仍在，
    /// 但新前端一律直接读本字段。
    pub whitelist_key: String,
    /// 同项目重复 dev server 的对端 PID（scan() 后处理填充；前端用于行内故事）
    pub duplicate_of: Option<u32>,
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
    /// 监听者的工作目录（仅监听 PID；macOS=lsof -d cwd，Windows=sysinfo cwd()）。
    /// 重复 dev server 检测的最强证据：monorepo 不同子包 / git worktree 的
    /// cwd 必然不同，同项目重复启动的 cwd 必然相同。读不到时优雅缺席。
    pub cwds: std::collections::HashMap<u32, String>,
    /// PID → 该进程**本地端**处于 ESTABLISHED 的端口集合（自动化实例的存活性证据）。
    /// mod.rs 与该 PID 的监听端口取交集 ⇒「调试端口上有客户端连着」。
    /// 采集口径按平台成本取舍：macOS 只对命令行呈现为自动化实例的 PID 再查一次
    /// lsof（日常零个 ⇒ 零开销，绝不放宽主 lsof 的 -sTCP:LISTEN —— 那会把全机
    /// 所有 TCP 连接拉进这次最贵的调用）；Windows 的 GetExtendedTcpTable 本就返回
    /// 全状态连接表，纯过滤条件改动、零额外成本，故全量填充。
    pub established_local_ports: std::collections::HashMap<u32, Vec<u16>>,
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
    /// 链在终止前走过至少一个真实祖先（合成根不算）。为 false 时链终止这件事
    /// 完全由 direct_orphan 决定，OrphanedChain 只是换句话重说一遍 ——
    /// 详见 mod.rs `ChainFlags::walked_real_ancestor`。**只影响理由的取舍，
    /// 不参与置信度分层**（分层读的是 chain_orphan，与本字段无关）。
    pub chain_walked_real_ancestor: bool,
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
    /// exe 路径本身是否落在标准安装位置（**未经类别例外修正**）。
    ///
    /// 与上一字段的差别正是「路径规则的两个例外」：dev-script 与
    /// automation-instance 的身份优先于路径，mod.rs 会把它们的
    /// `exe_is_standard_install` 压成 false 好让判定走到正向信号 —— 但那只说明
    /// 它们**没吃到路径豁免**，不代表 exe 真的装在非标准位置。
    /// `NonstandardPath` 是一条**说给用户听**的理由（i18n reasonTip 原文：
    /// 「可执行文件不在系统 / 应用程序等标准安装位置」），必须按事实推入，
    /// 否则 `/usr/bin/python3 app.py` 与 /Applications 里的 headless Chrome
    /// 都会被贴上一条与事实相反的证据（真机实测，两者的 exe 都在标准位置）。
    pub exe_path_is_standard: bool,
    /// 命令行命中 dev-server 关键字
    pub dev_keyword: bool,
    /// identify_app 类别为 dev-script
    pub dev_category: bool,
    /// identify_app 类别为 automation-instance —— 命令行呈现为一次性自动化浏览器
    /// 会话（--headless + 调试端口/临时 profile）。与 dev_category 同权参与判定，
    /// 且已在 mod.rs 被摘出路径豁免（浏览器本体常住 /Applications）。
    pub automation_instance: bool,
    /// 该自动化实例的调试端口上有活跃客户端连接 —— 存活性一票否决（只用于豁免）
    pub debugger_attached: bool,
}
