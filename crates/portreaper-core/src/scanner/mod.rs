//! 扫描编排：平台 provider 采集 → 信号快照（build_entry）→ 纯分类器（classify）
//! → 父链回溯（chain.rs）→ 跨条目后处理（postprocess.rs）→ 排序。
//! 消费方一律经 lib.rs 门面进入（`Scanner::scan` / `scan_once`）—— 桌面壳、
//! CLI、Raycast 共用同一入口，不存在自由函数 `scan()`。

mod chain;
mod classify;
mod identify;
mod model;
mod postprocess;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform_impl;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform_impl;

use std::collections::HashSet;

pub use model::ProcessEntry;

/// 供 platform::kill 的身份校验复用（macOS：kill 前用 `ps -o etime=` 重读创建时间）。
/// checked 版:解析失败返回 None → kill fail-closed,绝不把进程误当「刚启动」。
#[cfg(target_os = "macos")]
pub(crate) use macos::parse_etime_checked;

/// 供 platform::kill 复用同一份系统二进制绝对路径映射（kill/ps）—— 加固集中一处。
#[cfg(target_os = "macos")]
pub(crate) use macos::system_bin;

use chain::{build_parent_chain, is_pm2_container};
use classify::classify;
use identify::{basename, is_dev_server, AppIdentity};
use model::{Collected, ProcMeta, ProcessSnapshot};
use postprocess::{fill_subtree_cpu, mark_duplicates};

// identify_app 的类别全集，常量而非裸字面量 —— 判定、豁免、预闸、双平台阶梯
// 多处引用，改名不会漏改（AUTOMATION_CATEGORY 首创此规则，推广到全部类别）。
// wire 值由 model.rs 的契约注释与**测试里刻意保留的字面量**双重钉住：测试写
// 字面量正是为了让「悄悄改常量值」这种破坏 serde 契约的改动必然翻红。

/// 一次性自动化浏览器实例 —— 路径豁免的第二个例外（第一个是 dev-script）。
pub(crate) const AUTOMATION_CATEGORY: &str = "automation-instance";
/// 脚本运行时 / 开发期产物 —— 身份在脚本/模块/命令行，路径豁免的第一个例外。
pub(crate) const DEV_SCRIPT_CATEGORY: &str = "dev-script";
/// 用户安装的应用（/Applications、Program Files、Squirrel 布局等）—— 硬豁免。
pub(crate) const INSTALLED_APP_CATEGORY: &str = "installed-app";
/// 系统组件（/System、/usr/bin、SystemRoot 等）—— 硬豁免。
pub(crate) const SYSTEM_CATEGORY: &str = "system";
/// 用户目录 / 包管理器 CLI 的裸二进制 —— 位置不构成 dev 证据。
pub(crate) const USER_BINARY_CATEGORY: &str = "user-binary";
/// exe 不可读或不匹配任何阶梯的兜底。
pub(crate) const UNKNOWN_CATEGORY: &str = "unknown";

/// 白名单键：唯一标识「用户信任的这个监听者」，进程重启后仍要稳定匹配。
///
/// exe_path 含路径分隔符（绝对路径）时用它；否则是 PATH 解析的裸解释器名
/// （macOS `ps -o comm=` 对 `node app.js` / shebang shim 只返回裸 "node"）——
/// 此时 exe_path 在全机所有同名监听者间塌缩，单独加白一个 node server 会把
/// 所有 node 监听者一并豁免、令真孤儿永久隐身（评审发现）。裸名时回退到
/// 完整命令行（含脚本路径，足以区分不同 dev server）；命令行也空时退回 lsof 短名。
///
/// 引擎是本规则的**唯一实现**：所有前端（桌面 / CLI / Raycast）一律读
/// `ProcessEntry.whitelist_key`，不得自行重推（核心拆分刻意消灭的失败模式）。
/// 前端仅存 `legacyWhitelistKey`（v0.4.0 旧键兼容，引擎不产出该键）。
pub(crate) fn whitelist_key(exe_path: &str, full_command: &str, command: &str) -> String {
    if exe_path.contains('/') || exe_path.contains('\\') {
        exe_path.to_string()
    } else if !full_command.is_empty() {
        full_command.to_string()
    } else {
        command.to_string()
    }
}

/// v0.4.0 的旧键（exe_path 非空即用、否则 lsof 短名）。升级兼容用：v0.4.0 给
/// shebang / PATH 解析的脚本（exe_path==裸 "node"）存的就是这个裸键，新算法已改用
/// 完整命令行，旧键不再被任何进程命中 —— 若不兼容匹配，用户在 v0.4.0 加白的进程
/// 会在升级后重新变嫌疑、可能被「一键清扫」误杀（评审发现：无迁移的静默数据损失）。
/// 故 is_whitelisted 同时核对新旧键。旧裸键沿用 v0.4.0 的塌缩语义（命中全机同名
/// 监听者）—— 对一个 kill 工具，过度信任是安全方向，且本就是该用户升级前的行为；
/// 用户下次取消/重加该行即落为干净的新键。前端 src/model.ts legacyWhitelistKey 是镜像。
pub(crate) fn legacy_whitelist_key(exe_path: &str, command: &str) -> String {
    if !exe_path.is_empty() {
        exe_path.to_string()
    } else {
        command.to_string()
    }
}

/// CPU 百分比的采样策略 —— 只对 Windows 有实际影响。
///
/// Windows 的 `cpu_percent` 是 sysinfo **两次 refresh 之间**的增量：常驻 GUI 每
/// 2 秒轮询一次，采样区间由轮询本身天然提供（首屏显示 0% 是既定设计）。但一次性
/// 调用者（CLI / Raycast）冷启动只会 refresh 一次 —— 不预热的话**每一行的 CPU
/// 都恒为 0%**，而「这个残留进程正在烧 CPU」恰是用户最想看到的信息之一。
///
/// macOS 不受影响：`pcpu` 由 `ps` 直接给出，与采样区间无关。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuSampling {
    /// 不预热，最快。Windows 上 CPU 一律为 0%。
    Skip,
    /// 先刷一次进程表，等待 `interval` 后再正式采集。200ms 足够拿到可信读数。
    Interval(std::time::Duration),
}

impl Default for CpuSampling {
    fn default() -> Self {
        Self::Interval(std::time::Duration::from_millis(200))
    }
}

/// 一个可复用的扫描器 —— **持有平台状态**，连续调用之间的间隔即 Windows 的
/// CPU 采样区间。
///
/// 常驻前端（桌面 GUI）应当持有一个实例反复 `scan()`，语义与拆分前的进程级
/// `OnceLock<Mutex<System>>` 完全一致；一次性调用者用 [`scan_once`] 更省心。
pub struct Scanner {
    state: platform_impl::PlatformState,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: platform_impl::PlatformState::new(),
        }
    }

    /// 预热 CPU 采样基线（Windows 有效，macOS 是 no-op）。
    /// 通常不用手动调用 —— [`scan_once`] 会按 [`CpuSampling`] 代劳。
    pub fn warm_up(&mut self) {
        self.state.warm_up();
    }

    pub fn scan(&mut self, whitelist: &[String]) -> Vec<ProcessEntry> {
        let collected = self.state.collect();
        scan_from(collected, whitelist)
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

/// 一次性扫描：建临时 [`Scanner`]，按 `cpu` 决定要不要为 CPU 读数付预热成本。
///
/// 这是 CLI / 脚本的入口。常驻前端**不要**用它 —— 每次新建 Scanner 会丢掉
/// Windows 的采样区间，CPU 列会永远是 0%。
pub fn scan_once(whitelist: &[String], cpu: CpuSampling) -> Vec<ProcessEntry> {
    let mut scanner = Scanner::new();
    if let CpuSampling::Interval(d) = cpu {
        scanner.warm_up();
        std::thread::sleep(d);
    }
    scanner.scan(whitelist)
}

/// 纯编排：从一次已完成的平台采集产出最终行集合。与采集解耦，便于两个入口共用。
fn scan_from(collected: Collected, whitelist: &[String]) -> Vec<ProcessEntry> {
    let procs = &collected.procs;

    let mut entries = Vec::new();
    let mut seen: HashSet<u32> = HashSet::with_capacity(collected.listeners.len());

    // —— 主路径：监听端口的进程（lsof / 端口表）——
    for l in &collected.listeners {
        // lsof/端口表 与 进程表 是两次独立快照：拿不到元数据说明进程正在
        // 消失或刚出现 —— 丢弃该行（下个 2s 周期会补上）。这同时保证
        // start_unix 恒有值，kill 的身份校验永远不会因 null 失防（评审发现）。
        let Some(meta) = procs.get(&l.pid) else {
            continue;
        };
        seen.insert(l.pid);
        // 监听者的 user 优先取 lsof L 字段（更权威）；个别行缺 L 字段、或
        // Windows 端口表无 user 时回退进程表的 user 列。
        let user = if !l.user.is_empty() {
            l.user.clone()
        } else {
            meta.user.clone()
        };
        let (entry, _) = build_entry(
            EntryInput {
                pid: l.pid,
                meta,
                ports: l.ports.clone(),
                command: l.command.clone(),
                user,
                identity: None,
            },
            &collected,
            whitelist,
        );
        entries.push(entry);
    }

    // —— 第二路径：不占端口、但已脱离父进程的孤儿 dev 进程 ——
    //    数据源是同一份全进程表（macOS=ps -A / Windows=sysinfo），无额外系统调用：
    //    监听者只覆盖占端口的进程，被杀掉父进程的 dev 残留（如 electron-vite 中
    //    父 node 被杀、Electron 主进程被 launchd 收养成孤儿）既不占端口也就此漏网。
    //
    //    纳入门槛刻意比监听者更严：端口缺席时，dev-like 是「值得关注」的替代证据
    //    —— 否则全进程表里几十个正常的 ppid==1 系统 daemon 会全部涌入。classify
    //    的硬豁免（launchd / 标准路径 / brew / pm2）继续兜底防误报。
    for (&pid, meta) in procs {
        if seen.contains(&pid) {
            continue;
        }
        let command = basename(&meta.exe_path).to_string();
        // 廉价预闸（不回溯父链）：dev-like 是孤儿纳入的硬门槛，非 dev 进程直接跳过，
        // 避免对全进程表（数百个）逐个做 build_parent_chain 的父链回溯。
        // identify_app 结果传入 build_entry 复用，避免同一行算两遍路径阶梯；
        // full_command 为空时 build_entry 会以 command 兜底重算，此处不传以保等价。
        let identity = platform_impl::identify_app(&meta.full_command, &command, &meta.exe_path);
        if !orphan_gate_dev_like(&meta.full_command, &command, &identity.category) {
            continue;
        }
        let reusable_identity = (!meta.full_command.is_empty()).then_some(identity);
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid,
                meta,
                ports: Vec::new(),
                command,
                user: meta.user.clone(),
                identity: reusable_identity,
            },
            &collected,
            whitelist,
        );
        // 只纳入「判为嫌疑」的孤儿。白名单命中的孤儿（raw_suspect 为真但
        // is_zombie_suspect 已被扣为假）仍纳入，以便用户在列表里取消收藏；
        // 非嫌疑的健康 dev 进程（活终端里的 vite 等）不占端口、无残留意义，不进列表。
        if raw_suspect {
            entries.push(entry);
        }
    }

    // 跨条目后处理：同项目重复 dev server（classify 是单进程纯函数看不到全局）
    mark_duplicates(&mut entries, &collected.cwds);
    // 展示用后处理：子树 CPU 合计（不参与判定，见 ProcessEntry::cpu_percent_tree）
    fill_subtree_cpu(&mut entries, procs);

    // 排序：嫌疑优先 → 置信度高优先 → 端口号（孤儿无端口，端口键为 0 排在最前）
    entries.sort_by(|a, b| {
        b.is_zombie_suspect
            .cmp(&a.is_zombie_suspect)
            .then(b.confidence.cmp(&a.confidence))
            .then(
                a.ports
                    .first()
                    .copied()
                    .unwrap_or(0)
                    .cmp(&b.ports.first().copied().unwrap_or(0)),
            )
            // 端口相同（尤其无端口孤儿端口键全为 0）时按 pid 兜底，确保跨 poll 行序确定 ——
            // 否则孤儿遍历 HashMap 的随机序会渗入行序（评审 E1）。duplicate_of 的 peer
            // 选取由 mark_duplicates 内部的最小 PID 规则独立确定化，不依赖此排序。
            .then(a.pid.cmp(&b.pid))
    });

    entries
}

/// [`build_entry`] 的**逐行**输入（跨行共享的 `collected` / `whitelist` 单独传）。
///
/// 收成具名结构体而非位置参数：原先是 9 个位置参数，其中 `command` 与 `user`
/// 同为 `String` 且相邻 —— 调用点传反了编译器一声不吭，而 command 会一路流进
/// dev 关键字判定与白名单键。字段名即调用点的自证。
struct EntryInput<'a> {
    pid: u32,
    meta: &'a ProcMeta,
    /// 监听者传 lsof/端口表的端口；孤儿进程传空
    ports: Vec<u16>,
    /// 展示用短名：监听者取 lsof 的 command，孤儿取 exe basename
    command: String,
    user: String,
    /// 调用方已算好的 identify_app 结果（孤儿预闸顺手产出，传入复用）；
    /// None 时在此处计算（监听者路径）。
    identity: Option<AppIdentity>,
}

/// 从进程元数据构造一行 entry 及其判定 —— 监听者与孤儿进程共用，确保两条路径
/// 的孤儿判定零分叉。
///
/// 进程表取自 `collected.procs`，**不再单独收一个 `procs` 参数**：那是同一份数据
/// 的第二个引用，两者理论上可以不是同一张表，而这里的父链回溯与 direct_orphan
/// 必须与 `collected` 的其余通道（launchd_pids / established_local_ports）同源。
///
/// 返回 `(entry, raw_suspect)`：raw_suspect 是**未扣白名单**的 verdict.is_suspect
/// —— 孤儿循环据此决定是否纳入，使白名单命中的孤儿仍能显示以便取消收藏。
fn build_entry(
    input: EntryInput<'_>,
    collected: &Collected,
    whitelist: &[String],
) -> (ProcessEntry, bool) {
    let EntryInput {
        pid,
        meta,
        mut ports,
        command,
        user,
        identity,
    } = input;
    let procs = &collected.procs;
    let ppid = meta.ppid;
    let exe_path = meta.exe_path.clone();
    let full_command = if meta.full_command.is_empty() {
        command.clone()
    } else {
        meta.full_command.clone()
    };

    let AppIdentity {
        label: app_label,
        category: app_category,
    } = identity.unwrap_or_else(|| platform_impl::identify_app(&full_command, &command, &exe_path));

    let (parent_chain, chain_flags) = build_parent_chain(pid, procs);
    let launcher_label = parent_chain
        .last()
        .map(|p| p.label.clone())
        .unwrap_or_else(|| "?".to_string());

    // —— 豁免规则：installed-app/system 类别豁免；exe 在标准路径也豁免，
    //    但两个类别例外 —— 它们的身份不在路径里：
    //    · dev-script：脚本运行时的身份是脚本，不能因解释器装在系统目录
    //      （/usr/bin/python3、Program Files\nodejs）而漏报；
    //    · automation-instance：一次性自动化会话的身份是命令行，不能因浏览器
    //      本体装在 /Applications 而漏报（KNOWN-GAPS Gap 1，与上一条完全对称）。
    let identity_beats_path =
        app_category == DEV_SCRIPT_CATEGORY || app_category == AUTOMATION_CATEGORY;
    // 两个路径判断，语义**刻意不同**，不可互相替代（评审 8/9 个角度独立命中的坑）：
    //   · is_standard_install_path —— 豁免策略，刻意向 true 偏（macOS 收了
    //     /private/var/folders/ 给 App Translocation 让路，Windows 对读不到的
    //     空 exe 直接放行）。判定用它，宁可漏报不可误杀。
    //   · is_conventional_install_path —— 事实陈述，剔掉上述偏向。只喂给
    //     NonstandardPath 那条说给用户听的理由：拿豁免谓词陈述事实，它每放宽
    //     一次就多撒一次谎（`go run` 的临时产物正住在 /private/var/folders/）。
    let exe_path_is_standard = platform_impl::is_conventional_install_path(&exe_path);
    let exe_is_standard_install = app_category == INSTALLED_APP_CATEGORY
        || app_category == SYSTEM_CATEGORY
        || (platform_impl::is_standard_install_path(&exe_path) && !identity_beats_path);
    let brew_service_path = brew_service_exemption(&app_category, &full_command, &exe_path);

    // 自动化实例的存活性证据：调试端口（= 该 PID 的监听端口之一）上有 ESTABLISHED
    // 连接 ⇒ 有客户端正在驱动它。无端口的孤儿行 ports 为空 ⇒ 恒为 false —— 那是
    // `--remote-debugging-pipe`（stdio 通道）形态，其驱动者只能是父进程，
    // 父进程还活着时本就不成孤儿，故对判定无损。
    let automation_instance = app_category == AUTOMATION_CATEGORY;
    let debugger_attached = automation_instance
        && collected
            .established_local_ports
            .get(&pid)
            .is_some_and(|est| est.iter().any(|p| ports.contains(p)));

    let snapshot = ProcessSnapshot {
        state: meta.state.clone(),
        elapsed_secs: meta.elapsed_secs,
        direct_orphan: platform_impl::direct_orphan(ppid, meta, procs),
        chain_terminates_at_init: chain_flags.terminates_at_init,
        chain_has_orphan_shell: chain_flags.has_orphan_shell,
        chain_walked_real_ancestor: chain_flags.walked_real_ancestor,
        launchd_managed: collected.launchd_pids.contains(&pid),
        brew_service_path,
        pm2_managed: chain_flags.pm2 || is_pm2_container(&full_command),
        tty_orphaned: meta.tty_orphaned,
        exe_is_standard_install,
        exe_path_is_standard,
        dev_keyword: is_dev_server(&full_command) || is_dev_server(&command),
        dev_category: app_category == DEV_SCRIPT_CATEGORY,
        automation_instance,
        debugger_attached,
    };
    let verdict = classify(&snapshot);
    let raw_suspect = verdict.is_suspect;

    // 白名单 key（引擎唯一推导，前端直读 ProcessEntry.whitelist_key）。
    // 同时核对 v0.4.0 旧键以兼容升级（见 legacy_whitelist_key）。
    let wl_key = whitelist_key(&exe_path, &full_command, &command);
    let legacy_key = legacy_whitelist_key(&exe_path, &command);
    let is_whitelisted =
        whitelist.contains(&wl_key) || (legacy_key != wl_key && whitelist.contains(&legacy_key));

    ports.sort_unstable();

    let entry = ProcessEntry {
        pid,
        ppid,
        ports,
        command,
        full_command,
        exe_path,
        app_label,
        app_category,
        parent_chain,
        launcher_label,
        user,
        tty: meta.tty.clone().unwrap_or_default(),
        elapsed_secs: meta.elapsed_secs,
        start_unix: meta.start_unix,
        cpu_percent: meta.cpu_percent,
        // 占位：子树合计由 scan() 的后处理统一填充（需要全进程表的父子索引，
        // 逐行构建会退化成 O(行数 × 进程数) 的重复遍历）
        cpu_percent_tree: meta.cpu_percent,
        mem_mb: meta.rss_kb as f32 / 1024.0,
        state: meta.state.clone().unwrap_or_default(),
        is_zombie_suspect: verdict.is_suspect && !is_whitelisted,
        confidence: verdict.confidence,
        zombie_reasons: verdict.reasons,
        is_whitelisted,
        whitelist_key: wl_key,
        duplicate_of: None,
    };
    (entry, raw_suspect)
}

/// 第二条扫描路径（无端口孤儿）的纳入预闸：端口缺席时，dev-like 就是「值得关注」
/// 的替代证据 —— 否则全进程表里几十个正常的 ppid==1 系统 daemon 会全部涌入。
/// 刻意不回溯父链（全表逐行做 build_parent_chain 太贵），只看命令行与类别。
///
/// 抽成具名函数而非内联表达式，是为了让测试能**逐字复用同一个判据** ——
/// 在测试里重写一遍表达式必然随生产代码漂移，而这道闸正是 KNOWN-GAPS Gap 1
/// 路径二漏报的第一现场（gpu-process helper 当年就是在这里 `continue` 掉的）。
fn orphan_gate_dev_like(full_command: &str, command: &str, category: &str) -> bool {
    // 判据本体与 classify 的置信度分层共用同一个 is_dev_like（评审发现：曾是两份
    // 靠约定同步的内联表达式 —— 给 classify 新增 dev 信号时预闸不会自动跟上）。
    // automation-instance 同为「值得关注」的开发期产物：headless 浏览器的 helper
    // 子进程（--type=gpu-process 等）不占端口，主进程被杀后会被收养成孤儿 ——
    // 那正是本路径要接住的残留（KNOWN-GAPS Gap 1）。
    classify::is_dev_like(
        is_dev_server(full_command) || is_dev_server(command),
        category == DEV_SCRIPT_CATEGORY,
        category == AUTOMATION_CATEGORY,
    )
}

/// Homebrew 服务豁免按「身份路径」取证：dev-script 的身份是脚本/模块，
/// 不是解释器的安装位置 —— brew 装的 python/node 跑用户脚本或 `-m 模块`
/// 时不得享受服务豁免（真实漏报：孤儿 `python -m http.server`，解释器在
/// /opt/homebrew/Cellar/ 下被整体放行）。
/// 无脚本也无模块（REPL、console-script 包装如 supervisord）时保守沿用
/// 解释器路径 —— system-domain 的 brew python 服务仍受兜底保护。
fn brew_service_exemption(app_category: &str, full_command: &str, exe_path: &str) -> bool {
    // 自动化实例的身份是「一次性会话」，与浏览器二进制装在哪彻底无关 ——
    // brew 装的 chromium（/opt/homebrew/Cellar/…）跑 headless 时不得享受服务豁免，
    // 否则 Gap 1 的修复会在 brew 安装路径上留一个等价的漏洞。
    if app_category == AUTOMATION_CATEGORY {
        return false;
    }
    if app_category != DEV_SCRIPT_CATEGORY {
        return platform_impl::is_brew_service_path(exe_path);
    }
    match identify::extract_script_arg(full_command) {
        // 有脚本：豁免与否看脚本自己的位置（brew 包内脚本 → 豁免保留）
        Some(script) => platform_impl::is_brew_service_path(script),
        None => {
            identify::extract_module_arg(full_command).is_none()
                && platform_impl::is_brew_service_path(exe_path)
        }
    }
}

#[cfg(test)]
mod live_smoke {
    /// 真机冒烟（默认忽略，手动跑：cargo test live_scan -- --ignored --nocapture）：
    /// 对本机真实进程跑一遍完整管道，人工核对分类与豁免是否合理。
    #[test]
    #[ignore]
    fn live_scan() {
        // 走 scan_once + 预热：真机冒烟应当看到与 CLI 相同的读数（含 CPU）
        let entries = super::scan_once(&[], super::CpuSampling::default());
        // 「行」而非「监听者」：v0.6.0 起第二条扫描路径会带进无端口的孤儿 dev 进程
        let orphans = entries.iter().filter(|e| e.ports.is_empty()).count();
        println!(
            "\n==== live scan: {} rows ({} 无端口孤儿) ====",
            entries.len(),
            orphans
        );
        for e in &entries {
            println!(
                "{:>6}  :{:<24} {:<14} conf={:<9} reasons={:?}  [{}] {}",
                e.pid,
                e.ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                e.app_category,
                format!("{:?}", e.confidence),
                e.zombie_reasons,
                e.app_label,
                e.exe_path
            );
        }
    }
}

#[cfg(test)]
mod helper_tests {
    use super::*;

    #[test]
    fn whitelist_key_resolves_bare_interpreter_to_full_command() {
        // 绝对路径 exe：用 exe_path（向后兼容既有行为）
        assert_eq!(
            whitelist_key("/opt/homebrew/bin/node", "node app.js", "node"),
            "/opt/homebrew/bin/node"
        );
        assert_eq!(
            whitelist_key(
                "C:\\Program Files\\nodejs\\node.exe",
                "node x.js",
                "node.exe"
            ),
            "C:\\Program Files\\nodejs\\node.exe"
        );
        // 裸解释器名（PATH 解析 / shebang shim）：回退完整命令行，避免塌缩
        assert_eq!(
            whitelist_key("node", "node /Users/x/proj/server.js", "node"),
            "node /Users/x/proj/server.js"
        );
        // 裸名且命令行也空：退回 lsof 短名
        assert_eq!(whitelist_key("node", "", "node"), "node");
    }

    /// 升级兼容（评审发现）：v0.4.0 给 shebang/PATH 解析脚本存的是裸 exe 键，
    /// 新算法改用完整命令行 —— legacy_whitelist_key 必须复原旧键，is_whitelisted
    /// 才能继续命中、避免升级后已加白进程重新变嫌疑被误扫。
    #[test]
    fn legacy_whitelist_key_reproduces_v040_key() {
        // v0.4.0：exe_path 非空即用（裸 "node" 即旧键）
        assert_eq!(legacy_whitelist_key("node", "node"), "node");
        // 绝对路径：新旧键一致（无兼容负担）
        assert_eq!(
            legacy_whitelist_key("/opt/homebrew/bin/node", "node"),
            "/opt/homebrew/bin/node"
        );
        // exe 为空：退回 lsof 短名（与 v0.4.0 一致）
        assert_eq!(legacy_whitelist_key("", "node"), "node");

        // 端到端：用户在 v0.4.0 加白裸键 "node"，升级后仍应被识别为已加白
        let exe = "node";
        let full = "node /Users/x/proj/server.js";
        let new_key = whitelist_key(exe, full, "node");
        let legacy_key = legacy_whitelist_key(exe, "node");
        assert_ne!(new_key, legacy_key, "正是需要兼容匹配的场景");
        let v040_whitelist = ["node".to_string()];
        assert!(
            v040_whitelist.contains(&new_key)
                || (legacy_key != new_key && v040_whitelist.contains(&legacy_key)),
            "v0.4.0 的裸键必须仍被 is_whitelisted 命中"
        );
    }
}

#[cfg(all(test, target_os = "macos"))] // 端到端依赖 macOS 的 identify_app / direct_orphan
mod orphan_tests {
    use super::*;
    // 判定枚举只有测试用到（生产代码经 classify() 间接产出），故在此按需引入
    // 而非放顶层 use —— 顶层引入会在另一平台编译时变成未用 import 警告。
    use super::classify::{Confidence, ReasonCode};
    // 进程表只有夹具在直接建（生产侧一律经 collected.procs 拿），同理按需引入
    use std::collections::HashMap;

    // 夹具本体在 model.rs（ProcMeta::fixture / Collected::of_procs）——
    // 本模块只留同名薄别名，避免 ~20 个调用点的无谓 churn。
    fn meta(ppid: u32, exe: &str, cmd: &str) -> ProcMeta {
        ProcMeta::fixture(ppid, exe, cmd)
    }
    fn col_of(procs: HashMap<u32, ProcMeta>) -> Collected {
        Collected::of_procs(procs)
    }

    const ORPHAN_ELECTRON_EXE: &str =
        "/Users/x/proj/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";

    // —— scan_from 编排层（此前只测 build_entry 等零件，编排行为裸奔 —— 评审发现）——

    /// 监听者缺 meta 必须整行丢弃：这是「start_unix 恒有值 ⇒ kill 身份校验
    /// 永不因 null 失防」的安全前提，不只是显示问题。
    #[test]
    fn listener_without_meta_is_dropped() {
        let mut col = col_of(HashMap::new());
        col.listeners.push(model::Listener {
            pid: 4242,
            ports: vec![3000],
            user: "x".to_string(),
            command: "node".to_string(),
        });
        let entries = scan_from(col, &[]);
        assert!(
            entries.is_empty(),
            "缺 meta 的监听者必须丢行，不得以空身份进入列表"
        );
    }

    /// 同一 PID 双路径（既占端口又是孤儿 dev 进程）只出一行，且是监听者形态。
    #[test]
    fn same_pid_via_both_paths_emits_one_row() {
        let exe = ORPHAN_ELECTRON_EXE;
        let mut procs = HashMap::new();
        procs.insert(900, meta(1, exe, &format!("{exe} .")));
        let mut col = col_of(procs);
        col.listeners.push(model::Listener {
            pid: 900,
            ports: vec![5173],
            user: String::new(),
            command: "Electron".to_string(),
        });
        let entries = scan_from(col, &[]);
        assert_eq!(entries.len(), 1, "seen 去重失效：同 PID 出了两行");
        assert_eq!(entries[0].ports, vec![5173], "监听者路径优先，端口须保留");
    }

    /// 白名单孤儿在 scan 产出的列表里仍要出现（供用户取消收藏），但不算嫌疑。
    /// build_entry 层已有同名测试；这里钉的是 scan_from 的「raw_suspect 即纳入」。
    #[test]
    fn whitelisted_orphan_is_still_listed_by_scan_from() {
        let exe = ORPHAN_ELECTRON_EXE;
        let mut procs = HashMap::new();
        procs.insert(900, meta(1, exe, &format!("{exe} .")));
        let entries = scan_from(col_of(procs), &[exe.to_string()]);
        assert_eq!(entries.len(), 1, "白名单孤儿必须仍在列表里");
        assert!(entries[0].is_whitelisted);
        assert!(!entries[0].is_zombie_suspect);
    }

    /// 无端口孤儿（端口键全为 0）的行序必须按 pid 兜底确定 —— 孤儿遍历
    /// HashMap 的随机序不得渗入行序（评审 E1 的行序半边，此前只测了
    /// mark_duplicates 半边）。
    #[test]
    fn portless_orphan_rows_sort_by_pid_fallback() {
        // 两个不同项目的孤儿 dev 进程（避免被判成重复对），乱序插入
        let exe_a =
            "/Users/x/proj-a/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";
        let exe_b =
            "/Users/x/proj-b/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";
        let mut procs = HashMap::new();
        procs.insert(902, meta(1, exe_b, &format!("{exe_b} .")));
        procs.insert(901, meta(1, exe_a, &format!("{exe_a} .")));
        let entries = scan_from(col_of(procs), &[]);
        let pids: Vec<u32> = entries.iter().map(|e| e.pid).collect();
        assert_eq!(pids, vec![901, 902], "端口键同为 0 时必须按 pid 兜底排序");
    }

    /// 头号目标场景：electron-vite dev 中父 node 被杀，Electron 主进程被 launchd
    /// 收养成孤儿（ppid=1），不占任何端口、住在 node_modules 下。必须检出为
    /// Confirmed —— 这正是 portreaper「端口收割」盲区里最该清理的 dev 残留。
    #[test]
    fn orphan_electron_in_node_modules_is_confirmed() {
        let exe = "/Users/x/proj/node_modules/.pnpm/electron@33.4.11/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";
        let mut procs = HashMap::new();
        procs.insert(900, meta(1, exe, &format!("{exe} .")));
        let col = col_of(procs);
        let m = col.procs.get(&900).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 900,
                meta: m,
                ports: Vec::new(),
                command: "Electron".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );
        assert!(raw_suspect, "孤儿 Electron 必须判为嫌疑");
        assert!(entry.is_zombie_suspect);
        assert_eq!(entry.confidence, Confidence::Confirmed);
        assert_eq!(entry.app_category, "dev-script");
        assert!(entry.ports.is_empty(), "孤儿进程无端口");
        assert!(entry.zombie_reasons.contains(&ReasonCode::Ppid1Orphan));
        assert!(entry.zombie_reasons.contains(&ReasonCode::DevServerKeyword));
    }

    /// 对照（防误杀）：/Applications 里的 VS Code（也是 Electron）即便 ppid=1
    /// 也必须被 installed-app 豁免 —— node_modules 信号不得波及真安装的应用。
    #[test]
    fn installed_electron_app_in_applications_is_exempt() {
        let exe = "/Applications/Visual Studio Code.app/Contents/MacOS/Electron";
        let mut procs = HashMap::new();
        procs.insert(901, meta(1, exe, &format!("{exe} --type=renderer")));
        let col = col_of(procs);
        let m = col.procs.get(&901).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 901,
                meta: m,
                ports: Vec::new(),
                command: "Electron".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );
        assert!(!raw_suspect, "已安装应用即便 ppid=1 也不是孤儿嫌疑");
        assert!(!entry.is_zombie_suspect);
        assert_eq!(entry.app_category, "installed-app");
    }

    /// 端到端夹具：`/Applications` 下的进程按命令行取证。
    /// 三条一组，锁死 KNOWN-GAPS Gap 1 的整个判定边界 —— 三者的 exe **完全相同**，
    /// 区分它们的信息只在命令行与连接状态里。
    fn applications_chrome_entry(
        pid: u32,
        args: &str,
        ports: Vec<u16>,
        established: &[(u32, Vec<u16>)],
    ) -> (ProcessEntry, bool) {
        const EXE: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let mut procs = HashMap::new();
        procs.insert(pid, meta(1, EXE, &format!("{EXE} {args}")));
        let mut col = col_of(procs);
        col.established_local_ports = established.iter().cloned().collect();
        let m = col.procs.get(&pid).unwrap();
        build_entry(
            EntryInput {
                pid,
                meta: m,
                ports,
                command: "Google Chrome".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        )
    }

    /// Gap 1 主案（实测）：headless + 临时 profile + 调试端口，ppid=1，调试端口上
    /// 零 ESTABLISHED。exe 在 /Applications 却必须检出 —— 身份在命令行，不在路径。
    #[test]
    fn orphan_headless_automation_in_applications_is_detected() {
        let (entry, raw_suspect) = applications_chrome_entry(
            64834,
            "--headless=new --disable-gpu --user-data-dir=/private/tmp/claude-501/sess/cprof8 \
             --remote-debugging-port=9339 about:blank",
            vec![9339],
            &[],
        );
        assert!(raw_suspect, "无人认领的 headless 自动化实例必须判为嫌疑");
        assert!(entry.is_zombie_suspect);
        assert_eq!(entry.confidence, Confidence::Confirmed);
        assert_eq!(entry.app_category, AUTOMATION_CATEGORY);
        assert!(entry.zombie_reasons.contains(&ReasonCode::Ppid1Orphan));
        assert!(entry
            .zombie_reasons
            .contains(&ReasonCode::AutomationInstance));
        assert!(
            !entry.zombie_reasons.contains(&ReasonCode::InstalledApp),
            "不得再吃 /Applications 路径豁免"
        );
        // 摘出路径豁免 ≠ 路径变得非标准：exe 就在 /Applications 下，不得反过来
        // 贴一条「可执行文件不在标准安装位置」的错话（classify 侧不变量见
        // nonstandard_path_reason_follows_the_actual_exe_path）
        assert!(
            !entry.zombie_reasons.contains(&ReasonCode::NonstandardPath),
            "exe 在 /Applications 下，不得声称非标准路径：{:?}",
            entry.zombie_reasons
        );
    }

    /// 对照一（防误杀）：同一个 exe、同样 ppid=1，但**不带**自动化开关 ——
    /// 就是用户日常那个 Chrome，必须继续被 installed-app 豁免。
    #[test]
    fn plain_chrome_in_applications_stays_exempt() {
        let (entry, raw_suspect) = applications_chrome_entry(64000, "", vec![], &[]);
        assert!(!raw_suspect, "日常 Chrome 即便 ppid=1 也不是嫌疑");
        assert!(!entry.is_zombie_suspect);
        assert_eq!(entry.app_category, "installed-app");
        assert!(entry.zombie_reasons.contains(&ReasonCode::InstalledApp));
    }

    /// 对照二（Gap 1/A2 的实测反例，最危险的一条）：ppid=1 + 临时 profile +
    /// 调试端口，但**无 --headless**、且调试端口上有 ESTABLISHED 连接 ——
    /// 这是用户此刻正在驱动的活跃实例，误杀会打断一整个会话。
    ///
    /// 两道防线各自独立生效，任删其一都会误杀，故分别断言：
    ///   1. 无 --headless ⇒ 判据不成立，仍走 installed-app 豁免；
    ///   2. 即便命令行判据成立（加上 --headless），ESTABLISHED 也一票否决。
    #[test]
    fn live_driven_browser_instance_is_never_flagged() {
        // 防线 1：有头实例 —— 命令行判据的必要条件缺席
        let (entry, raw_suspect) = applications_chrome_entry(
            397,
            "--remote-debugging-port=9222 \
             --user-data-dir=/private/tmp/claude-501/sess/chrome-profile \
             --no-first-run --no-default-browser-check",
            vec![9222],
            &[(397, vec![9222])],
        );
        assert!(!raw_suspect, "有头的活跃实例必须豁免");
        assert_eq!(entry.app_category, "installed-app");

        // 防线 2：连 headless 都命中时，存活性否决兜底
        let (entry, raw_suspect) = applications_chrome_entry(
            398,
            "--headless=new --remote-debugging-port=9222 \
             --user-data-dir=/private/tmp/claude-501/sess/chrome-profile",
            vec![9222],
            &[(398, vec![9222])],
        );
        assert!(!raw_suspect, "调试端口有客户端连着 ⇒ 有人正在用，绝不标记");
        assert_eq!(entry.app_category, AUTOMATION_CATEGORY);
        assert!(entry.zombie_reasons.contains(&ReasonCode::DebuggerAttached));
    }

    /// 存活性证据必须落在**调试端口**上：一个正在抓网页的残留浏览器有大量出站
    /// ESTABLISHED（本地端是随机高位端口），那不是「有人在用它」——
    /// 若按「有任何连接就豁免」实现，Gap 1 的主案换个页面就重新漏报。
    #[test]
    fn outbound_connections_do_not_count_as_liveness() {
        let (entry, raw_suspect) = applications_chrome_entry(
            64835,
            "--headless=new --user-data-dir=/tmp/prof --remote-debugging-port=9339 \
             https://example.com",
            vec![9339],
            &[(64835, vec![54321, 54322])], // 出站连接的本地端口，与 9339 无交集
        );
        assert!(raw_suspect, "出站连接不构成存活性证据");
        assert!(entry.is_zombie_suspect);
        assert!(!entry.zombie_reasons.contains(&ReasonCode::DebuggerAttached));
    }

    /// KNOWN-GAPS Gap 1 的「真凶」那一支：headless 浏览器把 CPU 全烧在
    /// `--type=gpu-process` helper 子进程里，而 helper **不占任何端口** ——
    /// 主进程被杀后它被 launchd 收养成 ppid=1，正是第二条扫描路径该接住的残留。
    ///
    /// 端到端锁死整条链，三段各断言一次（当年三段各自都能让它漏网）：
    ///   1. `identify_app`：命令行取证归 automation-instance，不吃 /Applications 路径豁免；
    ///   2. `orphan_gate_dev_like`：预闸放行 —— helper 的命令行里没有任何 dev 关键字，
    ///      类别是它通过预闸的**唯一**理由，此前就是在这一步 `continue` 掉的；
    ///   3. `build_entry`：判为 Confirmed 且无端口。
    #[test]
    fn orphan_headless_helper_without_port_passes_gate_and_is_detected() {
        const HELPER: &str = "/Applications/Google Chrome.app/Contents/Frameworks/\
             Google Chrome Framework.framework/Helpers/Google Chrome Helper (GPU).app/\
             Contents/MacOS/Google Chrome Helper (GPU)";
        let full = format!(
            "{HELPER} --type=gpu-process --headless=new --use-gl=disabled \
             --user-data-dir=/private/tmp/claude-501/sess/scratchpad/cprof8"
        );
        let command = basename(HELPER).to_string();

        // 1. 归类
        let identity = platform_impl::identify_app(&full, &command, HELPER);
        assert_eq!(
            identity.category, AUTOMATION_CATEGORY,
            "helper 继承了主进程的自动化开关，身份同样在命令行"
        );

        // 2. 预闸（与 scan() 第二条路径逐字同一判据）
        assert!(
            !is_dev_server(&full) && !is_dev_server(&command),
            "前提：helper 命令行里没有任何 dev 关键字 —— 类别是它过闸的唯一理由"
        );
        assert!(
            orphan_gate_dev_like(&full, &command, &identity.category),
            "无端口的自动化 helper 必须过孤儿预闸"
        );

        // 3. 判定：主进程已死，helper 被 launchd 收养
        let mut procs = HashMap::new();
        procs.insert(64841, meta(1, HELPER, &full));
        let col = col_of(procs);
        let m = col.procs.get(&64841).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 64841,
                meta: m,
                ports: Vec::new(),
                command,
                user: String::new(),
                identity: Some(identity),
            },
            &col,
            &[],
        );
        assert!(raw_suspect, "无人认领的 headless helper 必须判为嫌疑");
        assert!(entry.is_zombie_suspect);
        assert_eq!(entry.confidence, Confidence::Confirmed);
        assert_eq!(entry.app_category, AUTOMATION_CATEGORY);
        assert!(entry.ports.is_empty(), "helper 不占端口");
        assert!(entry.zombie_reasons.contains(&ReasonCode::Ppid1Orphan));
        assert!(entry
            .zombie_reasons
            .contains(&ReasonCode::AutomationInstance));
        assert!(
            !entry.zombie_reasons.contains(&ReasonCode::InstalledApp),
            "不得再吃 /Applications 路径豁免"
        );
    }

    /// 对照（刻意的取舍，不是遗漏）：主进程还活着时，同一个满核 helper **不单独列行**
    /// —— 它 ppid 指向活着的主进程，父链又在 .app/ 处停住，没有任何孤儿信号。
    /// Gap 1 主案的这一形态由**主进程那一行** + 子树 CPU（fill_subtree_cpu →
    /// cpu_percent_tree，行内徽标）呈现，而不是把每个 helper 都摊成一行。
    #[test]
    fn busy_helper_under_live_parent_is_not_listed_separately() {
        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        const HELPER: &str = "/Applications/Google Chrome.app/Contents/Frameworks/\
             Google Chrome Framework.framework/Helpers/Google Chrome Helper (GPU).app/\
             Contents/MacOS/Google Chrome Helper (GPU)";
        let args = "--headless=new --user-data-dir=/private/tmp/sess/cprof8";
        let mut procs = HashMap::new();
        // 主进程活着（自身是 ppid=1 的孤儿 —— 它会由监听者那条路径单独标记）
        procs.insert(64834, meta(1, CHROME, &format!("{CHROME} {args}")));
        procs.insert(
            64841,
            meta(
                64834,
                HELPER,
                &format!("{HELPER} --type=gpu-process {args}"),
            ),
        );
        let col = col_of(procs);
        let m = col.procs.get(&64841).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 64841,
                meta: m,
                ports: Vec::new(),
                command: basename(HELPER).to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );
        assert!(
            !raw_suspect,
            "父进程健在 ⇒ 不是孤儿；helper 的负载由主进程行的子树 CPU 呈现"
        );
        assert!(!entry.is_zombie_suspect);
    }

    /// Playwright / Puppeteer 下载到缓存目录的浏览器：形态与 /Applications 里的
    /// 真应用完全相同（Chromium.app bundle），但它是项目的开发期 runtime ——
    /// 与 node_modules 下的 Electron.app 同一条不变量。
    #[test]
    fn orphan_playwright_browser_in_cache_is_detected() {
        let exe = "/Users/x/Library/Caches/ms-playwright/chromium-1148/chrome-mac/Chromium.app/Contents/MacOS/Chromium";
        let mut procs = HashMap::new();
        procs.insert(910, meta(1, exe, &format!("{exe} --no-startup-window")));
        let col = col_of(procs);
        let m = col.procs.get(&910).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 910,
                meta: m,
                ports: Vec::new(),
                command: "Chromium".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );
        assert!(raw_suspect, "工具下载的浏览器 runtime 孤儿必须检出");
        assert_eq!(entry.app_category, "dev-script");
        assert_eq!(entry.confidence, Confidence::Confirmed);
    }

    /// 白名单命中的孤儿仍返回 raw_suspect=true（以便纳入列表供用户取消收藏），
    /// 但 is_zombie_suspect 被扣为 false（不计入清扫 / 托盘）。
    #[test]
    fn whitelisted_orphan_still_surfaces_but_not_flagged() {
        let exe = "/Users/x/proj/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";
        let mut procs = HashMap::new();
        procs.insert(902, meta(1, exe, &format!("{exe} .")));
        let col = col_of(procs);
        let m = col.procs.get(&902).unwrap();
        let wl = vec![exe.to_string()]; // 绝对路径 exe → whitelist_key 即 exe_path
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 902,
                meta: m,
                ports: Vec::new(),
                command: "Electron".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &wl,
        );
        assert!(raw_suspect, "白名单孤儿仍需纳入列表");
        assert!(entry.is_whitelisted);
        assert!(!entry.is_zombie_suspect, "白名单命中后不标记嫌疑");
    }

    /// brew 豁免的「身份路径」矩阵：解释器位置 ≠ 进程身份。
    #[test]
    fn brew_exemption_follows_identity_path() {
        const BREW_PY: &str =
            "/opt/homebrew/Cellar/python@3.14/3.14.5/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python";

        // 孤儿 http.server 真实案例：brew 解释器 + `-m 模块` → 不豁免
        assert!(!brew_service_exemption(
            "dev-script",
            "Python -m http.server 8000",
            BREW_PY
        ));
        // brew 解释器跑用户脚本 → 不豁免（身份是脚本，脚本不在 brew）
        assert!(!brew_service_exemption(
            "dev-script",
            "python3 /Users/x/bot/main.py",
            BREW_PY
        ));
        // brew 包内脚本（python3 /opt/homebrew/libexec/foo.py）→ 豁免保留
        assert!(brew_service_exemption(
            "dev-script",
            "python3 /opt/homebrew/Cellar/somepkg/1.0/libexec/foo.py",
            BREW_PY
        ));
        // console-script 包装（supervisord，无扩展名无 -m）→ 保守沿用解释器路径
        assert!(brew_service_exemption(
            "dev-script",
            "python3 /opt/homebrew/bin/supervisord -c /opt/homebrew/etc/supervisord.conf",
            BREW_PY
        ));
        // 非 dev-script（postgres 等编译型服务）→ 维持原语义
        assert!(brew_service_exemption(
            "user-binary",
            "/opt/homebrew/opt/postgresql@16/bin/postgres -D /opt/homebrew/var/postgresql@16",
            "/opt/homebrew/opt/postgresql@16/bin/postgres"
        ));
        assert!(!brew_service_exemption(
            "user-binary",
            "/Users/x/go/bin/server",
            "/Users/x/go/bin/server"
        ));
    }
}

/// Windows 侧的端到端 fixture —— 与上面两个 macOS 模块对称。
///
/// 为什么单独写一份而不是把 macOS 那些改成平台中性：`build_entry` /
/// `build_parent_chain` 全程走 `platform_impl::*`，两个平台的孤儿判据、链终止
/// 条件、路径阶梯**没有一处相同**。这个 crate 的 Windows 半边没有任何手工 QA，
/// CI 是唯一的安全网 —— 而 CI 此前只跑得到纯函数 classify 与 windows.rs 的
/// 单元测试，从 ProcMeta 到 ProcessEntry 的这段组装逻辑在 Windows 上一行未测。
///
/// 路径取材刻意只用**全机一致**的位置（`C:\Program Files\`、`C:\Windows\`）与
/// 显然非标准的开发目录：runner 上的用户名、LOCALAPPDATA 都是变量，拿它们拼
/// fixture 会做成一条只在某台机器上成立的断言。
#[cfg(all(test, windows))]
mod windows_e2e_tests {
    use super::*;
    // 判定枚举只有测试用到（生产代码经 classify() 间接产出），故在此按需引入
    // 而非放顶层 use —— 顶层引入会在 macOS 编译时变成未用 import 警告。
    use super::classify::{Confidence, ReasonCode};
    // 进程表只有夹具在直接建（生产侧一律经 collected.procs 拿），同理按需引入
    use std::collections::HashMap;

    // 夹具本体在 model.rs；Windows 侧的 fixture 需要显式 start（槽位复用比较）。
    fn meta(ppid: u32, exe: &str, cmd: &str, start: u64) -> ProcMeta {
        let mut m = ProcMeta::fixture(ppid, exe, cmd);
        m.start_unix = Some(start);
        m
    }
    fn col_of(procs: HashMap<u32, ProcMeta>) -> Collected {
        Collected::of_procs(procs)
    }

    /// 存活的 explorer.exe 是链的合法终点。没有这一条，Windows 上每一个从资源
    /// 管理器/终端启动的 dev server 都会被判成孤儿链 —— 因为 Windows 的链最终
    /// 都会走到已退出的 userinit.exe（见 CLAUDE.md 的链走查不变量）。
    #[test]
    fn chain_stops_at_live_explorer() {
        let mut procs = HashMap::new();
        procs.insert(
            100,
            meta(1, "C:\\Windows\\explorer.exe", "explorer.exe", 1000),
        );
        procs.insert(
            200,
            meta(
                100,
                "C:\\Windows\\System32\\cmd.exe",
                "cmd.exe /c npm run dev",
                1100,
            ),
        );
        procs.insert(
            300,
            meta(
                200,
                "C:\\Program Files\\nodejs\\node.exe",
                "node C:\\dev\\proj\\node_modules\\vite\\bin\\vite.js",
                1200,
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        assert!(
            !flags.terminates_at_init,
            "活着的 explorer.exe 必须挡住孤儿链判定"
        );
        assert!(!flags.has_orphan_shell);
        assert!(flags.walked_real_ancestor);
        assert_eq!(
            chain.last().unwrap().pid,
            100,
            "链应停在 explorer.exe 这一层"
        );
    }

    /// 装在 `Program Files` 下的应用是 `is_chain_stopper`（category=installed-app）：
    /// 链走到它就停，不再继续上溯到已死的根。这是「用户正开着的 App 启动的进程
    /// 不算孤儿」那条不变量在 Windows 上的落点。
    #[test]
    fn chain_stops_at_installed_app_ancestor() {
        let mut procs = HashMap::new();
        // 祖先自身 ppid 指向一个不存在的 PID —— 若没有 installed-app 终止，
        // 链会继续上溯并落到「父不在快照 ⇒ 死根」分支
        procs.insert(
            100,
            meta(
                4242,
                "C:\\Program Files\\Microsoft VS Code\\Code.exe",
                "Code.exe",
                1000,
            ),
        );
        procs.insert(
            200,
            meta(
                100,
                "C:\\Program Files\\nodejs\\node.exe",
                "node C:\\dev\\proj\\server.js",
                1100,
            ),
        );

        let (chain, flags) = build_parent_chain(200, &procs);
        assert!(
            !flags.terminates_at_init,
            "链在 installed-app 处停下，不该被记成终止于死根"
        );
        assert!(flags.walked_real_ancestor);
        assert_eq!(chain.last().unwrap().pid, 100);
        assert_eq!(chain.last().unwrap().category, "installed-app");
    }

    /// PID 槽位复用（父存在、但创建时间晚于子 ⇒ 真实父早已死）走完整条组装链路：
    /// 非 dev 的用户二进制只到 Likely，且因 exe 不在常规安装位置带上 NonstandardPath。
    #[test]
    fn pid_slot_reused_yields_likely_with_nonstandard_path() {
        let exe = "C:\\dev\\tools\\myserver.exe";
        let mut procs = HashMap::new();
        // 父 50 的创建时间晚于子 900 ⇒ 槽位复用
        procs.insert(
            50,
            meta(1, "C:\\Windows\\System32\\cmd.exe", "cmd.exe", 9000),
        );
        procs.insert(900, meta(50, exe, "myserver.exe --serve", 1000));

        let col = col_of(procs);
        let m = col.procs.get(&900).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 900,
                meta: m,
                ports: vec![8080],
                command: "myserver.exe".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );

        assert!(raw_suspect, "槽位复用是直接孤儿信号");
        assert!(entry.is_zombie_suspect);
        assert!(entry.zombie_reasons.contains(&ReasonCode::PidSlotReused));
        assert!(
            entry.zombie_reasons.contains(&ReasonCode::NonstandardPath),
            "C:\\dev\\ 不是常规安装位置，该事实必须出现在证据里"
        );
        assert_eq!(
            entry.confidence,
            Confidence::Likely,
            "非 dev 的裸孤儿只到 Likely —— 升到 Confirmed 需要 dev 特征或死会话"
        );
    }

    /// 同一条路径上的 dev 特征会把置信度顶到 Confirmed（孤儿 × dev）——
    /// 与上一条构成对照，锁住 Windows 侧的分档确实生效而不是恒定一档。
    #[test]
    fn orphaned_dev_server_reaches_confirmed() {
        let mut procs = HashMap::new();
        procs.insert(
            900,
            meta(
                4242, // 父不在快照中 ⇒ ParentExited
                "C:\\Program Files\\nodejs\\node.exe",
                "node C:\\dev\\proj\\node_modules\\vite\\bin\\vite.js --port 5173",
                1000,
            ),
        );

        let col = col_of(procs);
        let m = col.procs.get(&900).unwrap();
        let (entry, raw_suspect) = build_entry(
            EntryInput {
                pid: 900,
                meta: m,
                ports: vec![5173],
                command: "node.exe".to_string(),
                user: String::new(),
                identity: None,
            },
            &col,
            &[],
        );

        assert!(raw_suspect);
        assert!(entry.zombie_reasons.contains(&ReasonCode::ParentExited));
        assert_eq!(
            entry.app_category, "dev-script",
            "解释器装在 Program Files，身份仍应取自脚本（路径规则例外 #1）"
        );
        assert_eq!(entry.confidence, Confidence::Confirmed);
    }
}
