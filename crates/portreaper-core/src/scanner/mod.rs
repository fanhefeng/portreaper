//! 扫描编排：平台 provider 采集 → 信号快照 → 纯分类器 → 父链 → 排序。
//! commands.rs 只依赖本文件的 `scan()` 与 `ProcessEntry`。

mod classify;
mod identify;
mod model;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform_impl;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform_impl;

use std::collections::{HashMap, HashSet};

pub use model::ProcessEntry;

/// 供 platform::kill 的身份校验复用（macOS：kill 前用 `ps -o etime=` 重读创建时间）。
/// checked 版:解析失败返回 None → kill fail-closed,绝不把进程误当「刚启动」。
#[cfg(target_os = "macos")]
pub(crate) use macos::parse_etime_checked;

/// 供 platform::kill 复用同一份系统二进制绝对路径映射（kill/ps）—— 加固集中一处。
#[cfg(target_os = "macos")]
pub(crate) use macos::system_bin;

use classify::{classify, is_dev_server, Confidence, ReasonCode};
use identify::basename;
use model::{Collected, ParentRef, ProcMeta, ProcessSnapshot};

/// 一次性自动化浏览器实例的类别名 —— 路径豁免的第二个例外（第一个是 dev-script）。
/// 常量而非裸字面量：判定、豁免、预闸三处引用，改名不会漏改（见 identify_app）。
pub(crate) const AUTOMATION_CATEGORY: &str = "automation-instance";

/// 父链回溯的同时收集的孤儿信号。
#[derive(Default)]
struct ChainFlags {
    /// 链走到 init/死根，途中无 installed-app、无存活系统根
    terminates_at_init: bool,
    /// 链上存在「自身已成孤儿」的 shell（死掉的终端会话）
    has_orphan_shell: bool,
    /// 链上存在 pm2 God Daemon
    pm2: bool,
    /// 链在终止前是否走过至少一个**真实**祖先（合成根 synth_chain_root 不算）。
    ///
    /// 为 false 时，「链终止于 init/死根」这件事完全由直接孤儿信号决定、不含任何
    /// 新信息：macOS 的 ppid==1 与 Windows 的 ppid==0 / 父不在表中，都在本函数
    /// 第一次迭代就命中终止分支 —— 而那三种情况恰好也正是两个平台的 direct_orphan
    /// 的全部触发条件。此时 OrphanedChain 只是把 Ppid1Orphan / ParentExited
    /// 换了句话再说一遍（评审发现：按 ReasonCode 变体特判会漏掉 Windows 这一半）。
    walked_real_ancestor: bool,
}

/// pm2 托管识别 —— 用「双标记并存」收紧裸子串误命中（评审发现）：单凭整行
/// 含 "PM2"（如 Java 类名 com.foo.PM2Handler）或目录恰名为 "ProcessContainer"
/// 就硬豁免，会让真孤儿漏报。pm2 实际形态唯一性足够：
///   God Daemon 标题恒为 `PM2 vX.Y.Z: God Daemon (...)`（两标记并存）；
///   被托管进程的包装器路径含 `.../pm2/.../ProcessContainer*`（pm2 + 容器名并存）。
fn is_pm2_god_daemon(cmd: &str) -> bool {
    cmd.contains("PM2") && cmd.contains("God Daemon")
}
fn is_pm2_container(cmd: &str) -> bool {
    cmd.contains("ProcessContainer") && cmd.contains("pm2")
}

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
            l.pid,
            meta,
            procs,
            &collected,
            whitelist,
            l.ports.clone(),
            l.command.clone(),
            user,
            None,
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
        if !orphan_gate_dev_like(&meta.full_command, &command, &identity.1) {
            continue;
        }
        let reusable_identity = (!meta.full_command.is_empty()).then_some(identity);
        let (entry, raw_suspect) = build_entry(
            pid,
            meta,
            procs,
            &collected,
            whitelist,
            Vec::new(),
            command,
            meta.user.clone(),
            reusable_identity,
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

/// 从进程元数据构造一行 entry 及其判定 —— 监听者与孤儿进程共用，确保两条路径
/// 的孤儿判定零分叉。监听者传 lsof 的 ports / command（短名）/ user；孤儿进程
/// 传空 ports、exe basename 作命令、ProcMeta 的 user。
///
/// `identity`：调用方已算好的 identify_app 结果（孤儿预闸顺手产出，传入复用）；
/// None 时在此处计算（监听者路径）。
///
/// 返回 `(entry, raw_suspect)`：raw_suspect 是**未扣白名单**的 verdict.is_suspect
/// —— 孤儿循环据此决定是否纳入，使白名单命中的孤儿仍能显示以便取消收藏。
#[allow(clippy::too_many_arguments)]
fn build_entry(
    pid: u32,
    meta: &ProcMeta,
    procs: &HashMap<u32, ProcMeta>,
    collected: &Collected,
    whitelist: &[String],
    mut ports: Vec<u16>,
    command: String,
    user: String,
    identity: Option<(String, String)>,
) -> (ProcessEntry, bool) {
    let ppid = meta.ppid;
    let exe_path = meta.exe_path.clone();
    let full_command = if meta.full_command.is_empty() {
        command.clone()
    } else {
        meta.full_command.clone()
    };

    let (app_label, app_category) =
        identity.unwrap_or_else(|| platform_impl::identify_app(&full_command, &command, &exe_path));

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
    let identity_beats_path = app_category == "dev-script" || app_category == AUTOMATION_CATEGORY;
    // 两个路径判断，语义**刻意不同**，不可互相替代（评审 8/9 个角度独立命中的坑）：
    //   · is_standard_install_path —— 豁免策略，刻意向 true 偏（macOS 收了
    //     /private/var/folders/ 给 App Translocation 让路，Windows 对读不到的
    //     空 exe 直接放行）。判定用它，宁可漏报不可误杀。
    //   · is_conventional_install_path —— 事实陈述，剔掉上述偏向。只喂给
    //     NonstandardPath 那条说给用户听的理由：拿豁免谓词陈述事实，它每放宽
    //     一次就多撒一次谎（`go run` 的临时产物正住在 /private/var/folders/）。
    let exe_path_is_standard = platform_impl::is_conventional_install_path(&exe_path);
    let exe_is_standard_install = app_category == "installed-app"
        || app_category == "system"
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
        dev_category: app_category == "dev-script",
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

/// 同项目重复 dev server 检测（跨条目后处理）。覆盖两类真实场景：
///   a) 完整命令逐字相同 —— 忘了已启动过，又跑了一遍同一条命令
///     （vite 会把第二个实例顺延到 3001，端口被白占）；
///   b) 同项目在不同终端 / IDE（Warp 起 5173、VS Code 起 5174）各起了一个实例
///     —— cwd 相同 + 脚本/模块身份相同，或路径推断的（项目, 脚本）一致。
///
/// cwd 是最强证据（评审发现）：monorepo 各子包 / git worktree 的 cwd 必然不同
/// （turbo 等编排器按包目录设 cwd），同项目重复启动的 cwd 必然相同 ——
/// 两侧 cwd 已知且不同 ⇒ 一票否决（hoisted node_modules 让路径推断的项目名
/// 全部坍缩到仓库根，仅靠路径会把 monorepo 的两个 app 误判成重复）。
///
/// 其余排除（全部有测试锁定）：
///   - 端口集相交：SO_REUSEPORT 多 worker 共享端口，不是重复；
///   - 互为父子：cluster master/worker；
///   - 真实存活的同一非 shell 父（concurrently / cluster master）⇒ 有意多实例；
///     父是 shell 或已死（合成 init 根）则照常比对 —— 同一终端重复跑两次、
///     双双被收养的孤儿对，正是要抓的场景（评审发现）；
///   - 共同祖父且祖父是存活的非 shell 编排器（turbo 经 shell 包装的堂兄弟）。
///     编排器证据**排除用户可见 App**（is_chain_stopper）：同一个 Terminal/iTerm
///     的两个 tab 各起一个 vite，共同祖父是终端 App 进程 —— 终端不是编排器，
///     tab 是独立会话，这正是要抓的重复（评审发现：终端祖父曾被误当 turbo 豁免）。
///
/// 不变量：重复信号只到 Possible，永不入清扫 —— 机器无法判断用户正在用哪个实例。
fn mark_duplicates(entries: &mut [ProcessEntry], cwds: &HashMap<u32, String>) {
    fn eligible(e: &ProcessEntry) -> bool {
        e.app_category == "dev-script"
            && !e.is_whitelisted
            // 被硬豁免的条目不参与：非嫌疑但带豁免原因（launchd/brew/pm2/标准路径）
            && (e.is_zombie_suspect || e.zombie_reasons.is_empty())
    }
    /// 脚本/模块身份：vite.js / http.server（b 档比对的一半）
    fn script_identity(e: &ProcessEntry) -> Option<String> {
        identify::extract_script_arg(&e.full_command)
            .map(|s| basename(s).to_string())
            .or_else(|| identify::extract_module_arg(&e.full_command).map(String::from))
    }
    /// 路径推断身份键：（项目名, 脚本）—— cwd 不可用时的回退
    fn project_key(e: &ProcessEntry) -> Option<(String, String)> {
        Some((
            identify::extract_project_name(&e.full_command)?,
            script_identity(e)?,
        ))
    }
    /// (pid, is_shell, is_user_visible_app)：后两者都排除「编排器」资格 ——
    /// shell 只是包装、用户可见 App（终端/IDE）不是编排器。
    fn chain_node(e: &ProcessEntry, depth: usize) -> Option<(u32, bool, bool)> {
        e.parent_chain.get(depth).map(|p| {
            (
                p.pid,
                platform_impl::is_shell(&p.exe_path),
                platform_impl::is_chain_stopper(&p.exe_path, &p.category),
            )
        })
    }
    /// peer 选取确定化（评审 E1 补全）：mark_duplicates 跑在 HashMap 迭代序上，
    /// ≥3 个重复实例时 get_or_insert 的「第一个匹配」会随轮询随机翻转，前端
    /// 「与 PID X 重复」闪变 —— 恒取最小 PID peer，与遍历顺序无关。
    fn assign_min(slot: &mut Option<u32>, pid: u32) {
        *slot = Some(slot.map_or(pid, |cur| cur.min(pid)));
    }

    // 预计算每个条目的派生身份:原实现在内层循环里对固定的 a 反复重算
    // project_key / script_identity(各含一次 split_whitespace 全命令行扫描)
    // 与 chain_node,是 O(n²)×解析。这里每条目只算一次,内层只做比较(评审 H1)。
    // 不 eligible 的条目留空,内层据 eligible 直接跳过。
    #[derive(Default)]
    struct Prep {
        eligible: bool,
        project: Option<(String, String)>,
        script_id: Option<String>,
        cwd: Option<String>,
        chain0: Option<(u32, bool, bool)>,
        chain1: Option<(u32, bool, bool)>,
    }
    let prep: Vec<Prep> = entries
        .iter()
        .map(|e| {
            if !eligible(e) {
                return Prep::default();
            }
            Prep {
                eligible: true,
                project: project_key(e),
                script_id: script_identity(e),
                cwd: cwds.get(&e.pid).cloned(),
                chain0: chain_node(e, 0),
                chain1: chain_node(e, 1),
            }
        })
        .collect();

    let n = entries.len();
    let mut peer: Vec<Option<u32>> = vec![None; n];
    for i in 0..n {
        if !prep[i].eligible {
            continue;
        }
        for j in (i + 1)..n {
            if !prep[j].eligible {
                continue;
            }
            let (a, b) = (&entries[i], &entries[j]);
            let (pi, pj) = (&prep[i], &prep[j]);
            // 互为父子（master/worker）
            if a.ppid == b.pid || b.ppid == a.pid {
                continue;
            }
            // 端口集相交（多 worker 共享端口）
            if a.ports.iter().any(|p| b.ports.contains(p)) {
                continue;
            }
            // cwd 一票否决：两侧已知且不同 ⇒ 不同子包/worktree/项目
            let (cwd_a, cwd_b) = (pi.cwd.as_deref(), pj.cwd.as_deref());
            if let (Some(ca), Some(cb)) = (cwd_a, cwd_b) {
                if ca != cb {
                    continue;
                }
            }
            // 真实存活的同一非 shell、非用户可见 App 父 ⇒ 编排器拉起的有意多实例；
            // 父是 shell / 用户可见 App / 已死（合成根 pid≤1）/ 链缺失 ⇒ 照常比对。
            // 两侧链都须独立印证该父（评审发现：只验 a 一侧、默认 b 链一致 ——
            // b 链缺失/形态不同时会被静默当作同编排器而漏标；收紧到双侧印证）。
            if a.ppid == b.ppid {
                if let (Some((pa, pa_sh, pa_app)), Some((pb, _, _))) = (pi.chain0, pj.chain0) {
                    if pa == a.ppid && pb == b.ppid && pa > 1 && !pa_sh && !pa_app {
                        continue;
                    }
                }
            }
            // 共同祖父的堂兄弟：祖父是存活的非 shell 编排器（turbo 经 shell 包装）。
            // 两侧 is_shell 都须为假（pid 相等时本是同进程、冗余但语义自证）；
            // 祖父是用户可见 App（同一终端两个 tab）不构成编排证据，照常比对。
            if let (Some((ga, ga_sh, ga_app)), Some((gb, gb_sh, _))) = (pi.chain1, pj.chain1) {
                if ga == gb && ga > 1 && !ga_sh && !gb_sh && !ga_app {
                    continue;
                }
            }
            let same_cmd = !a.full_command.is_empty() && a.full_command == b.full_command;
            let same_project = match (&pi.project, &pj.project) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            };
            let same_cwd = matches!((cwd_a, cwd_b), (Some(x), Some(y)) if x == y);
            let same_cwd_identity = same_cwd
                && matches!(
                    (&pi.script_id, &pj.script_id),
                    (Some(x), Some(y)) if x == y
                );
            // 路径/命令证据（same_cmd / same_project）在 hoisted node_modules 下会把
            // 不同 monorepo 子包坍缩成相同 —— 仅当 cwd 信息不是「一侧已知、一侧未知」
            // 时才采信（评审 M3：信息不对称时已知侧无从印证未知侧，易把子包误配对）。
            // 两侧都未知是纯路径回退（接受其风险）；两侧都已知到此必然相同（不同已被
            // 上面一票否决）。same_cwd_identity 本就要求双 cwd 相同，不受此限。
            let cwd_known = cwd_a.is_some() as u8 + cwd_b.is_some() as u8;
            let path_evidence_ok = cwd_known != 1;
            if same_cwd_identity || ((same_cmd || same_project) && path_evidence_ok) {
                assign_min(&mut peer[i], b.pid);
                assign_min(&mut peer[j], a.pid);
            }
        }
    }
    for (i, p) in peer.into_iter().enumerate() {
        let Some(pid) = p else { continue };
        let e = &mut entries[i];
        e.duplicate_of = Some(pid);
        e.zombie_reasons.push(ReasonCode::DuplicateDevServer);
        if !e.is_zombie_suspect {
            e.is_zombie_suspect = true;
            e.confidence = Confidence::Possible;
        }
    }
}

/// 第二条扫描路径（无端口孤儿）的纳入预闸：端口缺席时，dev-like 就是「值得关注」
/// 的替代证据 —— 否则全进程表里几十个正常的 ppid==1 系统 daemon 会全部涌入。
/// 刻意不回溯父链（全表逐行做 build_parent_chain 太贵），只看命令行与类别。
///
/// 抽成具名函数而非内联表达式，是为了让测试能**逐字复用同一个判据** ——
/// 在测试里重写一遍表达式必然随生产代码漂移，而这道闸正是 KNOWN-GAPS Gap 1
/// 路径二漏报的第一现场（gpu-process helper 当年就是在这里 `continue` 掉的）。
fn orphan_gate_dev_like(full_command: &str, command: &str, category: &str) -> bool {
    is_dev_server(full_command)
        || is_dev_server(command)
        || category == "dev-script"
        // 自动化实例同为「值得关注」的开发期产物：headless 浏览器的 helper 子进程
        // （--type=gpu-process 等）不占端口，主进程被杀后会被收养成孤儿 ——
        // 那正是本路径要接住的残留（KNOWN-GAPS Gap 1）。
        || category == AUTOMATION_CATEGORY
}

/// 子树 CPU 合计（展示用后处理）：把「自身 + 全部后代」的 pcpu 累加到被列出的那行。
///
/// 为什么必要（KNOWN-GAPS Gap 1/B 的真实荒诞点）：headless 浏览器主进程显示 ~0%，
/// 而它子树里的 `--type=gpu-process` 在满核空转 —— 只看行内 CPU，用户与判定链路
/// 都完全看不出异常。数据源是已采集的全进程表（ppid + cpu_percent），纯内存聚合、
/// 零额外系统调用；**不进 ProcessSnapshot、不参与判定**（健康的 vite build 一样满核）。
///
/// 一次构建父子索引后逐行 DFS：行数（几十）× 深度，远小于逐行全表扫描。
/// visited 兜住进程表快照里可能出现的自环 / 环路（父子创建瞬间的 ppid 竞态），
/// 否则 DFS 会死循环、整次扫描挂住（前端表现为 ERR_SCAN_TIMEOUT）。
fn fill_subtree_cpu(entries: &mut [ProcessEntry], procs: &HashMap<u32, ProcMeta>) {
    if entries.is_empty() {
        return;
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, meta) in procs {
        if meta.ppid != pid {
            children.entry(meta.ppid).or_default().push(pid);
        }
    }
    let mut visited: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = Vec::new();
    for e in entries.iter_mut() {
        visited.clear();
        stack.clear();
        // 根节点取行自身的值（entry 的 cpu_percent 就是它）—— 进程表里查不到自己
        // （两次快照的间隙）时优雅退化为自身 CPU，而不是把行清零。
        let mut total = e.cpu_percent;
        visited.insert(e.pid);
        stack.push(e.pid);
        while let Some(pid) = stack.pop() {
            let Some(kids) = children.get(&pid) else {
                continue;
            };
            for &kid in kids {
                if !visited.insert(kid) {
                    continue; // 环 / 重复入栈：每个节点只计一次
                }
                if let Some(meta) = procs.get(&kid) {
                    total += meta.cpu_percent;
                }
                stack.push(kid);
            }
        }
        e.cpu_percent_tree = total;
    }
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
    if app_category != "dev-script" {
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

/// 沿 PPID 向上回溯（≤12 层），同时收集孤儿链信号。
/// 停止条件：init（macOS=launchd，合成根节点）、第一个 installed-app
///（"这个 node 是 iTerm/Cursor 拉起的"）、存活的系统根（Windows explorer 等）、
/// 父缺失（Windows 死根，合成 System 节点）。
fn build_parent_chain(
    start_pid: u32,
    procs: &HashMap<u32, ProcMeta>,
) -> (Vec<ParentRef>, ChainFlags) {
    let mut chain = Vec::new();
    let mut flags = ChainFlags::default();
    let mut current_pid = start_pid;

    // 注：命中 installed-app / 存活系统根即 break，因此走到 init/死根分支时
    // 链上必然没有用户可见 App —— terminates_at_init 直接置 true 即可。

    // 死根收尾（两处共用）：Windows 补合成根并视为「链到 init」；macOS 同处
    // 只是 kernel(0) / 快照间隙的瞬态，保守收尾、不下结论。
    fn dead_root(chain: &mut Vec<ParentRef>, flags: &mut ChainFlags) {
        if cfg!(windows) {
            chain.push(platform_impl::synth_chain_root());
            flags.terminates_at_init = true;
        }
    }

    for _ in 0..12 {
        let Some(current) = procs.get(&current_pid) else {
            break;
        };
        let parent_ppid = current.ppid;

        // init：macOS 走到 launchd
        if platform_impl::chain_hits_init(parent_ppid) {
            chain.push(platform_impl::synth_chain_root());
            flags.terminates_at_init = true;
            break;
        }
        if parent_ppid == 0 || parent_ppid == current_pid {
            // 父未知/已退出（Windows）或走到 kernel(0)（macOS）
            dead_root(&mut chain, &mut flags);
            break;
        }
        let Some(parent) = procs.get(&parent_ppid) else {
            // 父进程已不在快照中
            dead_root(&mut chain, &mut flags);
            break;
        };

        let (label, category) = platform_impl::identify_app(
            &parent.full_command,
            basename(&parent.exe_path),
            &parent.exe_path,
        );

        // 存活的系统根（Windows explorer/services 等）：链的合法终点，非孤儿
        if platform_impl::is_live_session_root(&parent.exe_path) {
            chain.push(ParentRef {
                pid: parent_ppid,
                label,
                category,
                exe_path: parent.exe_path.clone(),
            });
            flags.walked_real_ancestor = true;
            break;
        }

        // 死掉的终端会话：链上的 shell 自身已是孤儿
        if platform_impl::is_shell(&parent.exe_path)
            && platform_impl::direct_orphan(parent.ppid, parent, procs).is_some()
        {
            flags.has_orphan_shell = true;
        }
        if is_pm2_god_daemon(&parent.full_command) {
            flags.pm2 = true;
        }

        let is_user_visible_app = platform_impl::is_chain_stopper(&parent.exe_path, &category);
        chain.push(ParentRef {
            pid: parent_ppid,
            label,
            category,
            exe_path: parent.exe_path.clone(),
        });
        flags.walked_real_ancestor = true;
        if is_user_visible_app {
            break;
        }
        current_pid = parent_ppid;
    }

    (chain, flags)
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

    #[test]
    fn pm2_detection_requires_both_markers() {
        // 真实 pm2 形态命中
        assert!(is_pm2_god_daemon("PM2 v6.0.5: God Daemon (/Users/x/.pm2)"));
        assert!(is_pm2_container(
            "node /usr/local/lib/node_modules/pm2/lib/ProcessContainerFork.js"
        ));
        // 误命中面：单标记不豁免（评审发现）
        assert!(!is_pm2_god_daemon("java -cp app.jar com.foo.PM2Handler"));
        assert!(!is_pm2_god_daemon("node /Users/x/God Daemon Sim/server.js"));
        assert!(!is_pm2_container("node /Users/x/ProcessContainer/index.js"));
    }
}

#[cfg(test)] // 平台中性：ProcessEntry 纯数据，is_shell 用 bash（双平台 shell 表都含）
mod dup_tests {
    use super::*;

    const VITE_A: &str = "node /Users/x/ai-portal/node_modules/vite/bin/vite.js dev --port 3000";

    fn entry(pid: u32, ppid: u32, ports: &[u16], cmd: &str) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid,
            ports: ports.to_vec(),
            command: "node".into(),
            full_command: cmd.into(),
            exe_path: "/opt/homebrew/bin/node".into(),
            app_label: String::new(),
            app_category: "dev-script".into(),
            parent_chain: vec![],
            launcher_label: String::new(),
            user: String::new(),
            tty: String::new(),
            elapsed_secs: 3600,
            start_unix: Some(1000),
            cpu_percent: 0.0,
            mem_mb: 0.0,
            state: String::new(),
            is_zombie_suspect: false,
            confidence: Confidence::None,
            zombie_reasons: vec![],
            is_whitelisted: false,
            // 夹具用固定 exe 路径，与 build_entry 的推导一致（含分隔符 ⇒ 用 exe_path）
            whitelist_key: "/opt/homebrew/bin/node".into(),
            duplicate_of: None,
            cpu_percent_tree: 0.0,
        }
    }

    fn parent(pid: u32, exe: &str) -> ParentRef {
        ParentRef {
            pid,
            label: basename(exe).to_string(),
            category: "unknown".into(),
            exe_path: exe.into(),
        }
    }

    fn no_cwd() -> HashMap<u32, String> {
        HashMap::new()
    }

    fn cwd_map(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs.iter().map(|(p, c)| (*p, c.to_string())).collect()
    }

    #[test]
    fn exact_command_duplicate_flagged_both_ways() {
        // ai-portal 真实案例：同命令、同 cwd、不同父链、端口被顺延
        let mut a = entry(88898, 88877, &[3000, 4206], VITE_A);
        a.parent_chain = vec![
            parent(88877, "/opt/homebrew/bin/node"),
            parent(88876, "/usr/local/bin/node"),
        ];
        let mut b = entry(46392, 46371, &[3001, 61405], VITE_A);
        b.parent_chain = vec![
            parent(46371, "/opt/homebrew/bin/node"),
            parent(46370, "/usr/local/bin/node"),
        ];
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[(88898, "/Users/x/ai-portal"), (46392, "/Users/x/ai-portal")]),
        );
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
        assert_eq!(es[0].confidence, Confidence::Possible);
        assert_eq!(es[0].duplicate_of, Some(46392));
        assert_eq!(es[1].duplicate_of, Some(88898));
        assert!(es[0]
            .zombie_reasons
            .contains(&ReasonCode::DuplicateDevServer));
    }

    #[test]
    fn same_project_different_launcher_flagged() {
        // 用户场景：Warp 起 5173、VS Code 起 5174 —— 命令参数不同但项目+脚本一致
        let a = entry(100, 10, &[5173], VITE_A);
        let b = entry(
            200,
            20,
            &[5174],
            "node /Users/x/ai-portal/node_modules/vite/bin/vite.js dev",
        );
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
        assert_eq!(es[0].duplicate_of, Some(200));
    }

    #[test]
    fn same_cwd_identity_flagged_outside_users() {
        // 项目不在 /Users 下（路径推断失效）：cwd + 脚本身份兜底
        let a = entry(100, 10, &[8080], "node /srv/proj/server.js");
        let b = entry(200, 20, &[8081], "node /srv/proj/server.js --verbose");
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &cwd_map(&[(100, "/srv/proj"), (200, "/srv/proj")]));
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn monorepo_apps_different_cwd_not_flagged() {
        // 评审发现：hoisted node_modules 下两个不同 app 的命令行可能逐字相同，
        // 路径推断的项目名也坍缩到仓库根 —— cwd 不同一票否决
        let cmd = "node /Users/x/mono/node_modules/vite/bin/vite.js dev";
        let a = entry(100, 10, &[3000], cmd);
        let b = entry(200, 20, &[3001], cmd);
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[
                (100, "/Users/x/mono/apps/web"),
                (200, "/Users/x/mono/apps/docs"),
            ]),
        );
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn worktrees_different_cwd_not_flagged() {
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[(100, "/Users/x/ai-portal"), (200, "/Users/x/ai-portal-wt2")]),
        );
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn asymmetric_cwd_path_evidence_not_flagged() {
        // 评审 M3:cwd「一侧已知、一侧未知」时,路径/命令证据(hoisted node_modules
        // 会把不同 monorepo 子包坍缩成逐字相同)不可信 —— 已知侧无从印证未知侧,
        // 不据此标重复,避免把子包误配对。两侧都未知(no_cwd)仍走纯路径回退。
        let cmd = "node /Users/x/mono/node_modules/vite/bin/vite.js dev";
        let a = entry(100, 10, &[3000], cmd);
        let b = entry(200, 20, &[3001], cmd);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &cwd_map(&[(100, "/Users/x/mono/apps/web")]));
        assert!(
            !es[0].is_zombie_suspect && !es[1].is_zombie_suspect,
            "cwd 信息不对称时路径证据不可信,不应标重复"
        );
    }

    #[test]
    fn cluster_workers_same_master_not_flagged() {
        // 同父且父是存活的编排器（node master）：有意的多实例
        let mut a = entry(100, 600, &[3000], VITE_A);
        a.parent_chain = vec![parent(600, "/opt/homebrew/bin/node")];
        let mut b = entry(200, 600, &[3001], VITE_A);
        b.parent_chain = vec![parent(600, "/opt/homebrew/bin/node")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn same_shell_run_twice_flagged() {
        // 同一个 shell 里后台跑了两次：正是要抓的重复
        let mut a = entry(100, 500, &[3000], VITE_A);
        a.parent_chain = vec![parent(500, "/bin/bash")];
        let mut b = entry(200, 500, &[3001], VITE_A);
        b.parent_chain = vec![parent(500, "/bin/bash")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn coreparented_orphan_siblings_flagged() {
        // 评审发现：双双被收养（ppid=1，链上是合成 init 根）的同命令对
        // 不能被「同父编排器」守卫吞掉 —— 父已死不构成编排证据
        let mut a = entry(100, 1, &[3000], VITE_A);
        a.parent_chain = vec![parent(1, "/sbin/launchd")];
        let mut b = entry(200, 1, &[3001], VITE_A);
        b.parent_chain = vec![parent(1, "/sbin/launchd")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn orchestrator_cousins_not_flagged() {
        // turbo 经 shell 包装拉起的堂兄弟：共同祖父是存活的编排器（非 shell）
        let mut a = entry(100, 601, &[3000], VITE_A);
        a.parent_chain = vec![parent(601, "/bin/sh"), parent(700, "/usr/local/bin/node")];
        let mut b = entry(200, 602, &[3001], VITE_A);
        b.parent_chain = vec![parent(602, "/bin/sh"), parent(700, "/usr/local/bin/node")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn terminal_app_grandparent_does_not_exempt() {
        // 评审发现：同一个 Terminal.app 的两个 tab 各直接 exec 了一遍 vite ——
        // 共同祖父是终端 App 进程（存活、非 shell），但终端不是编排器，
        // tab 是独立会话，必须照常标重复。用户可见 App（is_chain_stopper）
        // 不构成编排证据。category 用 installed-app 使双平台判定一致。
        fn term_parent(pid: u32) -> ParentRef {
            ParentRef {
                pid,
                label: "iTerm2".into(),
                category: "installed-app".into(),
                exe_path: "/Applications/iTerm.app/Contents/MacOS/iTerm2".into(),
            }
        }
        let mut a = entry(100, 501, &[5173], VITE_A);
        a.parent_chain = vec![parent(501, "/bin/zsh"), term_parent(900)];
        let mut b = entry(200, 502, &[5174], VITE_A);
        b.parent_chain = vec![parent(502, "/bin/zsh"), term_parent(900)];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(
            es[0].is_zombie_suspect && es[1].is_zombie_suspect,
            "终端 App 祖父不得被当作编排器豁免"
        );
        assert_eq!(es[0].confidence, Confidence::Possible);
    }

    #[test]
    fn peer_selection_deterministic_with_three_instances() {
        // 评审 E1 补全：≥3 个重复实例时 peer 必须与遍历顺序无关 ——
        // 恒为「除自己外的最小 PID」。两种入参顺序断言同一结果。
        for order in [[300u32, 100, 200], [100, 200, 300]] {
            let mut es: Vec<ProcessEntry> = order
                .iter()
                .map(|&pid| entry(pid, pid + 1000, &[(pid / 100) as u16 + 3000], VITE_A))
                .collect();
            mark_duplicates(&mut es, &no_cwd());
            for e in &es {
                let want = if e.pid == 100 { 200 } else { 100 };
                assert_eq!(
                    e.duplicate_of,
                    Some(want),
                    "pid {} 的 peer 应恒为最小对端（入参顺序 {:?}）",
                    e.pid,
                    order
                );
            }
        }
    }

    #[test]
    fn shared_port_workers_not_flagged() {
        // SO_REUSEPORT 多 worker 共享端口
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 20, &[3000, 3005], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn parent_child_not_flagged() {
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 100, &[3001], VITE_A); // b 是 a 的子进程
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn different_projects_same_script_not_flagged() {
        let a = entry(
            100,
            10,
            &[3000],
            "node /Users/x/blog/node_modules/vite/bin/vite.js dev",
        );
        let b = entry(
            200,
            20,
            &[3001],
            "node /Users/x/shop/node_modules/vite/bin/vite.js dev",
        );
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn whitelisted_excluded() {
        let mut a = entry(100, 10, &[3000], VITE_A);
        a.is_whitelisted = true;
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    /// 展示用的子树 CPU 聚合（KNOWN-GAPS Gap 1/B）：被列出的主进程行显示 ~0%，
    /// 而它的 gpu-process 子进程在满核 —— 合计必须落到那一行上，否则用户与
    /// 判定链路都看不出「空转 7 小时的进程树」和一个闲置进程有什么区别。
    /// 同时锁死环路防护：ppid 竞态造成的环不得让 DFS 死循环（会挂住整次扫描）。
    #[test]
    fn subtree_cpu_aggregates_children_and_survives_cycles() {
        fn proc(ppid: u32, cpu: f32) -> ProcMeta {
            ProcMeta {
                ppid,
                exe_path: String::new(),
                full_command: String::new(),
                user: String::new(),
                start_unix: Some(1000),
                elapsed_secs: 3600,
                cpu_percent: cpu,
                rss_kb: 0,
                tty: None,
                state: None,
                tty_orphaned: false,
            }
        }
        let mut procs = HashMap::new();
        procs.insert(100, proc(1, 0.4)); // headless 主进程：行内看着是闲的
        procs.insert(101, proc(100, 99.2)); // gpu-process：真凶
        procs.insert(102, proc(101, 1.0)); // 孙节点也要计入
        procs.insert(103, proc(104, 5.0)); // 环：103 ↔ 104
        procs.insert(104, proc(103, 7.0));

        let mut entries = vec![entry(100, 1, &[9339], VITE_A), entry(103, 104, &[], VITE_A)];
        entries[0].cpu_percent = 0.4; // 行自身就是「看着很闲」的那个数
        entries[1].cpu_percent = 5.0;
        fill_subtree_cpu(&mut entries, &procs);
        assert!(
            (entries[0].cpu_percent_tree - 100.6).abs() < 0.01,
            "主进程行应汇总整棵子树，实得 {}",
            entries[0].cpu_percent_tree
        );
        assert!(
            (entries[1].cpu_percent_tree - 12.0).abs() < 0.01,
            "环路必须收敛且每个节点只计一次，实得 {}",
            entries[1].cpu_percent_tree
        );
    }

    /// 进程表里查不到自己（lsof 与 ps 两次快照的间隙）时不得把行清零 ——
    /// 退化为自身 CPU 是最保守的展示语义。
    #[test]
    fn subtree_cpu_handles_missing_process_gracefully() {
        let mut entries = vec![entry(999, 1, &[3000], VITE_A)];
        entries[0].cpu_percent = 3.5;
        fill_subtree_cpu(&mut entries, &HashMap::new());
        assert_eq!(entries[0].cpu_percent_tree, 3.5, "退化为自身 CPU，不清零");
    }

    #[test]
    fn existing_suspect_keeps_confidence_gains_reason() {
        // 一个本来就是孤儿 Confirmed：置信度不降级，只追加重复信号
        let mut a = entry(100, 1, &[3000], VITE_A);
        a.is_zombie_suspect = true;
        a.confidence = Confidence::Confirmed;
        a.zombie_reasons = vec![ReasonCode::Ppid1Orphan];
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert_eq!(es[0].confidence, Confidence::Confirmed);
        assert!(es[0]
            .zombie_reasons
            .contains(&ReasonCode::DuplicateDevServer));
        assert_eq!(es[0].duplicate_of, Some(200));
        assert_eq!(es[1].confidence, Confidence::Possible);
    }
}

#[cfg(all(test, target_os = "macos"))] // 端到端依赖 macOS 的 identify_app / direct_orphan
mod orphan_tests {
    use super::*;

    fn meta(ppid: u32, exe: &str, cmd: &str) -> ProcMeta {
        ProcMeta {
            ppid,
            exe_path: exe.to_string(),
            full_command: cmd.to_string(),
            user: String::new(),
            start_unix: Some(1000),
            elapsed_secs: 3600,
            cpu_percent: 0.0,
            rss_kb: 0,
            tty: None,
            state: None,
            tty_orphaned: false,
        }
    }

    // ProcMeta 非 Clone：Collected 直接 move 持有 procs，build_entry 借 col.procs。
    fn col_of(procs: HashMap<u32, ProcMeta>) -> Collected {
        Collected {
            listeners: vec![],
            procs,
            launchd_pids: HashSet::new(),
            cwds: HashMap::new(),
            established_local_ports: HashMap::new(),
        }
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
            900,
            m,
            &col.procs,
            &col,
            &[],
            Vec::new(),
            "Electron".to_string(),
            String::new(),
            None,
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
            901,
            m,
            &col.procs,
            &col,
            &[],
            Vec::new(),
            "Electron".to_string(),
            String::new(),
            None,
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
            pid,
            m,
            &col.procs,
            &col,
            &[],
            ports,
            "Google Chrome".to_string(),
            String::new(),
            None,
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
            identity.1, AUTOMATION_CATEGORY,
            "helper 继承了主进程的自动化开关，身份同样在命令行"
        );

        // 2. 预闸（与 scan() 第二条路径逐字同一判据）
        assert!(
            !is_dev_server(&full) && !is_dev_server(&command),
            "前提：helper 命令行里没有任何 dev 关键字 —— 类别是它过闸的唯一理由"
        );
        assert!(
            orphan_gate_dev_like(&full, &command, &identity.1),
            "无端口的自动化 helper 必须过孤儿预闸"
        );

        // 3. 判定：主进程已死，helper 被 launchd 收养
        let mut procs = HashMap::new();
        procs.insert(64841, meta(1, HELPER, &full));
        let col = col_of(procs);
        let m = col.procs.get(&64841).unwrap();
        let (entry, raw_suspect) = build_entry(
            64841,
            m,
            &col.procs,
            &col,
            &[],
            Vec::new(),
            command,
            String::new(),
            Some(identity),
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
            64841,
            m,
            &col.procs,
            &col,
            &[],
            Vec::new(),
            basename(HELPER).to_string(),
            String::new(),
            None,
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
            910,
            m,
            &col.procs,
            &col,
            &[],
            Vec::new(),
            "Chromium".to_string(),
            String::new(),
            None,
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
            902,
            m,
            &col.procs,
            &col,
            &wl,
            Vec::new(),
            "Electron".to_string(),
            String::new(),
            None,
        );
        assert!(raw_suspect, "白名单孤儿仍需纳入列表");
        assert!(entry.is_whitelisted);
        assert!(!entry.is_zombie_suspect, "白名单命中后不标记嫌疑");
    }
}

#[cfg(all(test, target_os = "macos"))] // 链 fixture 全部基于 macOS 进程形态
mod chain_tests {
    use super::*;

    fn meta(ppid: u32, exe: &str, cmd: &str) -> ProcMeta {
        ProcMeta {
            ppid,
            exe_path: exe.to_string(),
            full_command: cmd.to_string(),
            user: String::new(),
            start_unix: Some(1000),
            elapsed_secs: 600,
            cpu_percent: 0.0,
            rss_kb: 0,
            tty: None,
            state: None,
            tty_orphaned: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn orphan_chain_zsh_npm_vite() {
        // vite(300) ← npm(200) ← zsh(100, ppid=1 已被收养) —— 头号漏报场景
        let mut procs = HashMap::new();
        procs.insert(100, meta(1, "/bin/zsh", "-zsh"));
        procs.insert(200, meta(100, "/opt/homebrew/bin/node", "npm run dev"));
        procs.insert(
            300,
            meta(
                200,
                "/opt/homebrew/bin/node",
                "node /Users/x/proj/node_modules/.bin/vite",
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        assert!(flags.terminates_at_init, "链应终止于 launchd");
        assert!(flags.has_orphan_shell, "链上应识别出孤儿 zsh");
        assert!(
            flags.walked_real_ancestor,
            "走过 npm、zsh 才撞到 launchd ⇒ 链是一份独立证据"
        );
        // 链：npm → zsh → launchd
        assert_eq!(chain.last().unwrap().label, "launchd");
    }

    /// 锁住 classify 的 OrphanedChain 去重所依赖的**前提**（评审发现：此前只有
    /// 纯函数侧断言了结论，没有任何测试 pin 住产生该前提的这次遍历）。
    ///
    /// 前提是：本体 ppid==1 时，遍历从自己起步、第一次迭代就命中 chain_hits_init，
    /// 一个真实祖先都没走过。若将来有人把起点改成 meta.ppid、或在 chain_hits_init
    /// 之前插入别的终止分支，这条断言会先红 —— 否则 classify 会继续默默吞掉一条
    /// 此时已经变得独立的 OrphanedChain 证据，而全部纯函数测试照样全绿。
    #[cfg(target_os = "macos")]
    #[test]
    fn ppid1_leaf_terminates_before_walking_any_ancestor() {
        let mut procs = HashMap::new();
        procs.insert(
            400,
            meta(1, "/opt/homebrew/bin/node", "node /Users/x/proj/server.js"),
        );
        let (chain, flags) = build_parent_chain(400, &procs);
        assert!(flags.terminates_at_init, "ppid=1 ⇒ 链终止于 launchd");
        assert!(
            !flags.walked_real_ancestor,
            "ppid=1 时第一次迭代即终止，不得走过任何真实祖先"
        );
        assert_eq!(chain.len(), 1, "链上只有合成的 launchd 根");
        assert_eq!(chain[0].label, "launchd");
    }

    /// brew 豁免的「身份路径」矩阵：解释器位置 ≠ 进程身份。
    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
    #[test]
    fn live_terminal_chain_not_orphan() {
        // vite(300) ← zsh(200) ← Terminal.app(100, 活着)
        let mut procs = HashMap::new();
        procs.insert(
            100,
            meta(
                1,
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            ),
        );
        procs.insert(200, meta(100, "/bin/zsh", "-zsh"));
        procs.insert(
            300,
            meta(
                200,
                "/opt/homebrew/bin/node",
                "node /Users/x/proj/node_modules/.bin/vite",
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        // Terminal.app 虽在 /System/ 下（类别 system），但 is_chain_stopper 按
        // ".app/" 识别为用户可见 App —— 链在此停下，不会误判为孤儿链。
        assert!(!flags.terminates_at_init, "活终端必须挡住孤儿链判定");
        assert!(!flags.has_orphan_shell);
        assert_eq!(chain.last().unwrap().label, "Terminal");
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

    fn meta(ppid: u32, exe: &str, cmd: &str, start: u64) -> ProcMeta {
        ProcMeta {
            ppid,
            exe_path: exe.to_string(),
            full_command: cmd.to_string(),
            user: String::new(),
            start_unix: Some(start),
            elapsed_secs: 3600,
            cpu_percent: 0.0,
            rss_kb: 0,
            tty: None,
            state: None,
            tty_orphaned: false,
        }
    }

    fn col_of(procs: HashMap<u32, ProcMeta>) -> Collected {
        Collected {
            listeners: vec![],
            procs,
            launchd_pids: HashSet::new(),
            cwds: HashMap::new(),
            established_local_ports: HashMap::new(),
        }
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
            900,
            m,
            &col.procs,
            &col,
            &[],
            vec![8080],
            "myserver.exe".to_string(),
            String::new(),
            None,
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
            900,
            m,
            &col.procs,
            &col,
            &[],
            vec![5173],
            "node.exe".to_string(),
            String::new(),
            None,
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
