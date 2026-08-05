//! macOS 数据采集与路径规则。
//! 三个数据源：lsof（监听套接字）、ps（全进程元数据；会话首进程靠 state 的
//! 's' 标志识别）、launchctl（托管 PID 集合）。
//! 全部子进程强制 en_US.UTF-8，避免本地化输出破坏列解析。

use std::collections::{HashMap, HashSet};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::classify::ReasonCode;
use super::identify::{basename, is_script_runtime, project_binary_label, script_runtime_label};
use super::model::{Collected, Listener, ParentRef, ProcMeta};

// 系统组件路径前缀 —— /System、/Library、/usr 等下的可执行文件视为系统组件。
// identify_app 的「系统组件」归类（step 2c）与 is_standard_install_path 的豁免共用
// 这一份，避免两处列表 drift（评审 E2）。/Applications/ 与 /private/var/folders/
// 不在此列：前者归 installed-app（step 2a），后者要让 dev-build 产物先被 step 5 接住。
const SYSTEM_COMPONENT_PREFIXES: &[&str] = &[
    "/System/",
    "/Library/",
    "/usr/libexec/",
    "/usr/sbin/",
    "/usr/bin/",
    "/sbin/",
    "/bin/",
];

/// Homebrew 服务路径 —— brew services 启动的守护（postgres/redis/...）的兜底豁免。
/// launchctl list 只能看到当前用户 GUI domain，system-domain 的服务靠这个路径特征兜底。
const BREW_SERVICE_PREFIXES: &[&str] = &[
    "/opt/homebrew/opt/",
    "/opt/homebrew/Cellar/",
    "/usr/local/opt/",
    "/usr/local/Cellar/",
];

const SHELLS: &[&str] = &["zsh", "bash", "sh", "fish", "dash", "csh", "tcsh"];

pub(crate) fn is_standard_install_path(exe_path: &str) -> bool {
    // 豁免全集 = 系统组件 + /Applications/（installed-app）+ App Translocation 临时目录
    exe_path.starts_with("/Applications/")
        || exe_path.starts_with("/private/var/folders/")
        || SYSTEM_COMPONENT_PREFIXES
            .iter()
            .any(|p| exe_path.starts_with(p))
}

/// 「这个 exe 确实装在常规安装位置吗」—— **陈述事实**，供 NonstandardPath 那条
/// 说给用户听的理由取证。刻意与 `is_standard_install_path` 分开：那个是**豁免策略**，
/// 会刻意向 true 偏（它把 `/private/var/folders/` 也算进去，为 App Translocation
/// 让路），而临时目录恰恰是「非常规安装位置」最成立的场景 —— `go run` 的临时编译
/// 产物就住在那里，识别为 dev-script 后正需要这条证据。
///
/// 与 identify.rs `is_temp_dir_path` 同源的教训（那里的注释写着「两者语义相反，
/// 绝不能共用同一个函数」）：豁免谓词的每一次放宽都是为了少杀人，拿它陈述事实，
/// 放宽一次就多撒一次谎。
pub(crate) fn is_conventional_install_path(exe_path: &str) -> bool {
    exe_path.starts_with("/Applications/")
        || SYSTEM_COMPONENT_PREFIXES
            .iter()
            .any(|p| exe_path.starts_with(p))
}

pub(crate) fn is_brew_service_path(exe_path: &str) -> bool {
    BREW_SERVICE_PREFIXES
        .iter()
        .any(|p| exe_path.starts_with(p))
}

pub(crate) fn is_shell(exe_path: &str) -> bool {
    let name = basename(exe_path).trim_start_matches('-');
    SHELLS.contains(&name)
}

/// 直接孤儿：macOS 上孤儿会被立即收养到 launchd（PID 1）。
pub(crate) fn direct_orphan(
    ppid: u32,
    _meta: &ProcMeta,
    _procs: &HashMap<u32, ProcMeta>,
) -> Option<ReasonCode> {
    (ppid == 1).then_some(ReasonCode::Ppid1Orphan)
}

/// 父链根的合成节点：链走到 PID 1 时挂一个 launchd 节点。
pub(crate) fn synth_chain_root() -> ParentRef {
    ParentRef {
        pid: 1,
        label: "launchd".to_string(),
        category: "system".to_string(),
        exe_path: "/sbin/launchd".to_string(),
    }
}

/// 链走到 PID 1 即 init；macOS 没有 Windows 那种「存活系统根」概念。
pub(crate) fn chain_hits_init(parent_ppid: u32) -> bool {
    parent_ppid == 1
}

pub(crate) fn is_live_session_root(_exe_path: &str) -> bool {
    false
}

/// 链回溯的「用户可见 App」终点：installed-app 之外还包括任何 .app bundle ——
/// 系统自带 Terminal.app 位于 /System/Applications/（类别 system），
/// 若不在它处停下，链会一路走到 launchd，把活终端里的 dev server 误报成孤儿链。
pub(crate) fn is_chain_stopper(exe_path: &str, category: &str) -> bool {
    category == "installed-app" || exe_path.contains(".app/")
}

/// (label, category) —— macOS 路径阶梯。顺序敏感：脚本/模块身份 → .app →
/// /Applications 裸 → 系统 → 裸脚本运行时 → Homebrew CLI → cargo 产物 →
/// 用户目录 → unknown。脚本/模块必须最先判：解释器自身可能就住在
/// .app bundle / 系统路径里（Python.app、/usr/bin/python3）。
pub(crate) fn identify_app(
    full_command: &str,
    short_command: &str,
    exe_path: &str,
) -> (String, String) {
    let exe = exe_path;

    // 0. 脚本/模块身份优先于一切路径与 .app 判定 —— brew / python.org 的
    //    Python 都以 .../Python.app/Contents/MacOS/Python 形态存在，若先走
    //    .app 阶梯会被归 installed-app 豁免（真实漏报：孤儿 python -m
    //    http.server 占着 8000 端口，解释器在 Cellar 的 Python.app 里）。
    if is_script_runtime(short_command) {
        if let Some(script) = super::identify::extract_script_arg(full_command) {
            if is_standard_install_path(script) {
                // 系统自带的脚本任务（/System/.../foo.py）仍归系统
                return (basename(script).to_string(), "system".to_string());
            }
            return (
                script_runtime_label(full_command, short_command),
                "dev-script".to_string(),
            );
        }
        // `-m 模块` 调用：身份是模块（python -m http.server）
        if let Some(module) = super::identify::extract_module_arg(full_command) {
            return (
                format!("{} · {}", module, short_command),
                "dev-script".to_string(),
            );
        }
    }

    // 0b. 一次性自动化浏览器实例 —— 身份在命令行，不在路径（与阶梯 0 的脚本身份
    //     完全对称）。必须先于 .app / /Applications 阶梯：headless Chrome 的宿主
    //     可执行文件就住在 /Applications，被归 installed-app 即吃硬豁免、永远漏网
    //     （KNOWN-GAPS Gap 1 的真实案例：空转 7 小时、子进程满核）。
    if super::identify::is_automation_instance(full_command) {
        return (
            super::identify::automation_label(exe, short_command),
            super::AUTOMATION_CATEGORY.to_string(),
        );
    }

    // 1. .app bundle —— 抽出 .app 名（exe 来自 ps comm，含空格也完整）
    if let Some(idx) = exe.find(".app/") {
        let before = &exe[..idx];
        if let Some(slash) = before.rfind('/') {
            let app_name = &before[slash + 1..];
            // 开发工具自带 / 下载的 .app 是项目本地的开发 runtime —— electron 把
            // Electron.app 装在 node_modules/electron/dist、Playwright 把 Chromium.app
            // 下载到 ~/Library/Caches/ms-playwright，形态与 /Applications 里的真应用
            // 一模一样。它们不是用户安装的应用，不能享受 installed-app 豁免，否则被杀掉
            // 父进程的孤儿 dev runtime 会因「长得像已安装应用」永远漏网。
            // 用户安装的应用绝不会住在这些目录里，故此信号零误伤（判定见 identify.rs）。
            if super::identify::is_dev_tool_runtime_path(exe) {
                return (app_name.to_string(), "dev-script".to_string());
            }
            let category = if exe.starts_with("/System/") || exe.starts_with("/Library/") {
                "system"
            } else {
                "installed-app"
            };
            return (app_name.to_string(), category.to_string());
        }
    }

    // 2a. /Applications/ 下的裸二进制
    if exe.starts_with("/Applications/") {
        return (basename(exe).to_string(), "installed-app".to_string());
    }

    // 2c. 系统组件（与 is_standard_install_path 共用 SYSTEM_COMPONENT_PREFIXES）
    if SYSTEM_COMPONENT_PREFIXES.iter().any(|p| exe.starts_with(p)) {
        return (basename(exe).to_string(), "system".to_string());
    }

    // 3. 无脚本参数的脚本运行时（node REPL、python -m 等）—— 按 exe 走原阶梯
    if is_script_runtime(short_command) {
        return (
            script_runtime_label(full_command, short_command),
            "dev-script".to_string(),
        );
    }

    // 4. /usr/local/, /opt/homebrew/, /opt/local/ → 用户安装的 CLI
    if exe.starts_with("/usr/local/")
        || exe.starts_with("/opt/homebrew/")
        || exe.starts_with("/opt/local/")
    {
        return (basename(exe).to_string(), "user-binary".to_string());
    }

    // 5. Rust/Cargo 产物、`go run` 临时编译产物（/private/var/folders/.../go-build*/exe/main）——
    //    必须先于路径豁免给出 dev-script 身份，否则 /private/var/folders/ 的标准路径前缀会把
    //    孤儿 go run 服务整体豁免（评审发现的真实漏报；该前缀本为 App Translocation 设，
    //    而那些路径含 .app/ 早被阶梯 1 接住）。判定片段集中在 identify::is_dev_build_artifact。
    if super::identify::is_dev_build_artifact(exe) {
        return (project_binary_label(exe), "dev-script".to_string());
    }

    // 6. /Users/... → 用户目录下的自定义二进制。
    //    注意类别是 user-binary 而非 dev-script：「位于用户目录」只说明位置，
    //    不构成 dev 证据 —— dev-script 会把裸孤儿二进制直升 Confirmed 入清扫。
    if exe.starts_with("/Users/") {
        return (project_binary_label(exe), "user-binary".to_string());
    }

    // 7. fallback
    let bin = basename(exe);
    let label = if bin.is_empty() {
        short_command.to_string()
    } else {
        bin.to_string()
    };
    (label, "unknown".to_string())
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

/// 系统工具固定绝对路径调用（纵深防御）：不经继承的 $PATH 解析，避免被排在
/// /bin 之前的可写目录里的同名二进制劫持。对一个会代用户做破坏性操作（kill）
/// 的工具，钉死系统二进制位置是零成本加固（评审发现）。未知名回退裸名。
/// pub(crate)：kill 路径（platform.rs）共用同一份映射，避免 "kill"/"ps" 的绝对
/// 路径在两处各写一遍而漂移 —— 一处加固、另一处仍可被劫持（评审发现）。
pub(crate) fn system_bin(program: &str) -> &str {
    match program {
        "lsof" => "/usr/sbin/lsof",
        "ps" => "/bin/ps",
        "launchctl" => "/bin/launchctl",
        "kill" => "/bin/kill",
        other => other,
    }
}

fn cmd_output(program: &str, args: &[&str]) -> Option<String> {
    let output = match Command::new(system_bin(program))
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            // 采集层失败不能静默退化为空输出（与 windows.rs 的留痕对齐）：
            // ps 失败 ⇒ 表格凭空清空、launchctl 失败 ⇒ 托管豁免失效，
            // 没有留痕时用户与开发者都拿不到任何线索（评审发现）。
            log::warn!("{program} {args:?} failed to spawn: {e}; scan may be degraded");
            return None;
        }
    };
    // 留痕但不丢弃 stdout：lsof 在「零结果」与「-p 列表中个别 PID 已消失」时
    // 都返回非零退出码，但 stdout 仍是可用的（部分）结果 —— 按非零整体丢弃
    // 会把其余监听者的 cwd 一并清掉。只在 stderr 有实际内容时记录。
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            log::warn!(
                "{program} {args:?} exited with {}: {stderr}; scan may be degraded",
                output.status
            );
        }
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 一次扫描会话的平台状态 —— macOS 侧无需持有任何东西。
///
/// 与 Windows 的 [`super::windows::PlatformState`] 保持相同的形状（编译期多态，
/// 无 trait）：那边持有 `sysinfo::System` 是因为 CPU 百分比来自两次 refresh 的
/// 增量；这边的 `pcpu` 由 `ps` 每次直接给出，天生无状态，故 `warm_up` 是 no-op。
pub(crate) struct PlatformState;

impl PlatformState {
    pub(crate) fn new() -> Self {
        Self
    }

    /// macOS 的 CPU 读数不依赖采样区间，无需预热。
    pub(crate) fn warm_up(&mut self) {}
}

impl PlatformState {
    pub(crate) fn collect(&mut self) -> Collected {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let listeners = parse_lsof(
            &cmd_output("lsof", &["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-FpcLn"])
                .unwrap_or_default(),
        );
        let comm_map =
            parse_comm(&cmd_output("ps", &["-A", "-o", "pid=,comm="]).unwrap_or_default());
        let procs = parse_ps(
            &cmd_output(
                "ps",
                &[
                    "-A",
                    "-o",
                    "pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,user=,command=",
                ],
            )
            .unwrap_or_default(),
            &comm_map,
            now,
        );
        let launchd_pids = parse_launchctl(&cmd_output("launchctl", &["list"]).unwrap_or_default());

        // 仅查监听者的 cwd（一次 lsof，~15 个 PID）：重复 dev server 检测的证据
        let pid_csv = listeners
            .iter()
            .map(|l| l.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let cwds = if pid_csv.is_empty() {
            HashMap::new()
        } else {
            parse_cwd(
                &cmd_output("lsof", &["-a", "-p", &pid_csv, "-d", "cwd", "-Fpn"])
                    .unwrap_or_default(),
            )
        };

        // 自动化实例的存活性证据（KNOWN-GAPS Gap 1/A2）：调试端口只 LISTEN、零
        // ESTABLISHED 才是真正的「无人认领」。**只对命令行呈现为自动化实例的 PID**
        // 再查一次 lsof —— 日常这个集合为空 ⇒ 零额外开销；刻意不放宽上面那次
        // `-sTCP:LISTEN` 过滤：那会把全机所有 TCP 连接拉进本项目最贵的一次调用。
        let automation_csv = procs
            .iter()
            .filter(|(_, m)| super::identify::is_automation_instance(&m.full_command))
            .map(|(pid, _)| pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let established_local_ports = if automation_csv.is_empty() {
            HashMap::new()
        } else {
            parse_established(
                &cmd_output(
                    "lsof",
                    &[
                        "-a",
                        "-p",
                        &automation_csv,
                        "-iTCP",
                        "-sTCP:ESTABLISHED",
                        "-P",
                        "-n",
                        "-Fpn",
                    ],
                )
                .unwrap_or_default(),
            )
        };

        Collected {
            listeners,
            procs,
            launchd_pids,
            cwds,
            established_local_ports,
        }
    }
}

/// `lsof … -sTCP:ESTABLISHED -Fpn` 输出：pPID 行后跟 n 行
/// `127.0.0.1:9222->127.0.0.1:54191`。取 **`->` 左侧（本地端）** 的端口 ——
/// 调用方与该 PID 的监听端口取交集，才是「有人连着它的调试端口」。
/// 右侧（对端）端口若被误当本地端口，一个正在抓网页的残留浏览器会因大量
/// 出站连接被误判成「有人在用」而豁免（本函数存在的全部意义就是不出这个错）。
fn parse_established(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            // 解析失败必须清空（与 parse_lsof / parse_cwd 同一底线）：
            // 否则后续 n 行会归到上一个 PID 名下，把别人的连接算作它的存活证据
            cur = rest.parse().ok();
        } else if let Some(addr) = line.strip_prefix('n') {
            // split_once 而非 split().next()：后者对无 `->` 的行也返回整串（永不为
            // None），校验只能靠另写一次 contains —— 一个读起来像守卫、实际永不触发的
            // 分支。这里让「必须有对端」直接由解析本身表达：无 `->` 的是监听行，
            // 不是连接，不构成存活证据。
            let Some((local, _peer)) = addr.split_once("->") else {
                continue;
            };
            if let (Some(pid), Some(port)) = (
                cur,
                local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()),
            ) {
                let ports = map.entry(pid).or_default();
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    map
}

/// `lsof -a -p <pids> -d cwd -Fpn` 输出：pPID 行后跟 n<路径> 行。
fn parse_cwd(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            // parse 失败必须清空而非保留旧值：否则后续 n 行会归到上一个 PID
            // 名下（cwd 张冠李戴）。实践中 p 行恒为数字，守的是解析器底线。
            cur = rest.parse().ok();
        } else if let Some(path) = line.strip_prefix('n') {
            if let Some(pid) = cur {
                map.insert(pid, path.to_string());
            }
        }
    }
    map
}

/// lsof -F 字段前缀模式：p=PID c=命令 L=用户 n=地址。一个 PID 多端口合并。
fn parse_lsof(text: &str) -> Vec<Listener> {
    let mut by_pid: HashMap<u32, Listener> = HashMap::new();
    let mut current_pid: Option<u32> = None;

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let tag = line.as_bytes()[0];
        // get(1..) 而非 [1..]：后者是字节切片，若行首是多字节 UTF-8 字符会 panic。
        // lsof -F 每行以 ASCII tag 开头、实践中不会触发，但本文件其余解析器全用
        // 边界安全 API（strip_prefix/split_whitespace），不留这个孤点（评审发现）。
        let Some(rest) = line.get(1..) else {
            continue;
        };
        match tag {
            b'p' => {
                // parse 失败清空而非保留旧值：否则后续 c/L/n 行会归到上一个
                // 进程名下（端口/用户张冠李戴）。实践中 p 行恒为数字，
                // 守的是「解析器不信任输入」底线（与多字节防御同标准）。
                current_pid = rest.parse::<u32>().ok();
                if let Some(pid) = current_pid {
                    by_pid.entry(pid).or_insert(Listener {
                        pid,
                        command: String::new(),
                        user: String::new(),
                        ports: vec![],
                    });
                }
            }
            b'c' => {
                if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                    e.command = rest.to_string();
                }
            }
            b'L' => {
                if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                    e.user = rest.to_string();
                }
            }
            b'n' => {
                let addr = rest.split("->").next().unwrap_or(rest);
                if let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                    if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                        if !e.ports.contains(&port) {
                            e.ports.push(port);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    by_pid.into_values().collect()
}

/// `ps -A -o pid=,comm=`：comm 是最后一列，整行截取 —— 含空格的 exe 路径也完整。
/// 这是 exe_path 的权威来源（修复旧版按空格截断 "/Applications/Visual Studio Code.app" 的 bug）。
fn parse_comm(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some((pid_str, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if let Ok(pid) = pid_str.parse::<u32>() {
            map.insert(pid, rest.trim().to_string());
        }
    }
    map
}

/// 解析 [[dd-]hh:]mm:ss 形式的 etime 为秒。**任一段不是合法数字 ⇒ None**(解析失败)。
/// kill 路径据此 fail-closed:解析失败绝不能静默当成「0 秒 / 刚启动」—— 那会把一个
/// 真实创建时间很早的进程算成 now,从而绕过 PID 复用容差(评审发现:与本文件
/// 其余解析器的 fail-closed 风格保持一致)。
pub(crate) fn parse_etime_checked(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        [s] => (0, 0, *s),
        _ => return None,
    };
    Some(days * 86_400 + h * 3_600 + m * 60 + sec)
}

/// snapshot 的 elapsed_secs 用:解析失败退化为 0(=「刚启动」=落入 grace 宽限,
/// 偏保守不误扫)。kill 路径请改用 parse_etime_checked 走 fail-closed。
pub(crate) fn parse_etime(s: &str) -> u64 {
    parse_etime_checked(s).unwrap_or(0)
}

/// 解析 ps 输出。数值列在 command 之前；command 是最后一列收尾全行。
///
/// 会话首进程的判定用 state 的 `s` 标志（BSD ps：「s = session leader」）——
/// **不要**用 `ps -o sess=`：macOS 上它对所有进程恒输出 0（内核会话指针对
/// 非特权调用方不可见），曾导致 leader 集合恒空、所有终端进程被误判
/// tty_orphaned（评审捕获的真机级 bug）。
fn parse_ps(text: &str, comm_map: &HashMap<u32, String>, now: u64) -> HashMap<u32, ProcMeta> {
    struct Row {
        pid: u32,
        meta: ProcMeta,
    }
    let mut rows: Vec<Row> = Vec::new();

    for line in text.lines() {
        let line = line.trim_start();
        let mut iter = line.split_whitespace();
        let Some(pid) = iter.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ppid) = iter.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let state = iter.next().unwrap_or("").to_string();
        let tty = iter.next().unwrap_or("?").to_string();
        let elapsed_secs = parse_etime(iter.next().unwrap_or("0"));
        let cpu_percent: f32 = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let rss_kb: u64 = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        // macOS 账户短名不允许空格，单 token 安全；监听者仍优先用 lsof L 字段，
        // 这一列主要补齐「无端口孤儿」行的 user（此前恒空，评审发现）。
        let user = iter.next().unwrap_or("").to_string();
        let full_command: String = iter.collect::<Vec<_>>().join(" ");

        // exe 权威来源：comm（含空格完整）；退回命令行首 token
        let exe_path = comm_map.get(&pid).cloned().unwrap_or_else(|| {
            full_command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        });

        rows.push(Row {
            pid,
            meta: ProcMeta {
                ppid,
                exe_path,
                full_command,
                user,
                start_unix: Some(now.saturating_sub(elapsed_secs)),
                elapsed_secs,
                cpu_percent,
                rss_kb,
                tty: Some(tty),
                state: Some(state),
                tty_orphaned: false,
            },
        });
    }

    // 仍有会话首进程（state 含 's'）的 tty 集合 —— 真机验证：每个活跃 ttys
    // 恰有一个带 's' 标志的进程（登录 shell 的 "Ss"/"Ss+"）
    let leader_ttys: HashSet<String> = rows
        .iter()
        .filter(|r| r.meta.state.as_deref().is_some_and(|st| st.contains('s')))
        .filter_map(|r| r.meta.tty.as_deref())
        .filter(|t| t.starts_with("ttys"))
        .map(String::from)
        .collect();

    let mut map = HashMap::new();
    for mut row in rows {
        if let Some(tty) = row.meta.tty.as_deref() {
            // 有真实终端、但该终端已无会话首进程 ⇒ 终端会话已死（iTerm 崩溃等）
            if tty.starts_with("ttys") && !leader_ttys.contains(tty) {
                row.meta.tty_orphaned = true;
            }
        }
        map.insert(row.pid, row.meta);
    }
    map
}

/// `launchctl list` 输出：PID\tStatus\tLabel；首列为 "-" 表示未运行。
/// 能解析出数字 PID 的行即「launchd 当前托管的进程」。
fn parse_launchctl(text: &str) -> HashSet<u32> {
    text.lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|t| t.parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etime_formats() {
        assert_eq!(parse_etime("05"), 5);
        assert_eq!(parse_etime("01:02"), 62);
        assert_eq!(parse_etime("01:02:03"), 3723);
        assert_eq!(parse_etime("2-01:02:03"), 2 * 86400 + 3723);
    }

    #[test]
    fn etime_checked_rejects_garbage() {
        // 合法输入与旧版同值
        assert_eq!(parse_etime_checked("01:02:03"), Some(3723));
        assert_eq!(parse_etime_checked("2-01:02:03"), Some(2 * 86400 + 3723));
        // 任一段非法数字 ⇒ None(kill 路径据此 fail-closed,绝不退化为 0)
        assert_eq!(parse_etime_checked(""), None);
        assert_eq!(parse_etime_checked("oops"), None);
        assert_eq!(parse_etime_checked("01:xx"), None);
        assert_eq!(parse_etime_checked("z-01:02"), None);
        // 旧版仍对垃圾静默退化为 0(parse_ps 的 grace 宽限用,偏保守)
        assert_eq!(parse_etime("oops"), 0);
    }

    #[test]
    fn comm_map_keeps_spaces() {
        let text =
            "  123 /Applications/Visual Studio Code.app/Contents/MacOS/Electron\n  456 /bin/zsh\n";
        let map = parse_comm(text);
        assert_eq!(
            map.get(&123).unwrap(),
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
        );
        assert_eq!(map.get(&456).unwrap(), "/bin/zsh");
    }

    #[test]
    fn lsof_field_mode() {
        let text =
            "p123\ncnode\nLfhf\nn*:5173\nn127.0.0.1:5174\np456\ncpostgres\nLfhf\nn127.0.0.1:5432\n";
        let mut ls = parse_lsof(text);
        ls.sort_by_key(|l| l.pid);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].pid, 123);
        assert_eq!(ls[0].command, "node");
        assert_eq!(ls[0].ports, vec![5173, 5174]);
        assert_eq!(ls[1].ports, vec![5432]);
    }

    /// 回归（评审发现）：行首是多字节 UTF-8 字符时不得 panic ——
    /// 旧实现 `&line[1..]` 按字节切片，切在续字节上直接崩掉整次扫描。
    /// lsof -F 实践中行首恒为 ASCII tag，此处守住的是「解析器不信任输入」底线。
    #[test]
    fn lsof_multibyte_leading_line_is_skipped_not_panicking() {
        let text = "p123\ncnode\n中文噪声行\nn*:5173\n";
        let ls = parse_lsof(text);
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].ports, vec![5173]);
    }

    /// 回归（评审发现）：p 行解析失败必须清空 current_pid —— 否则其后的
    /// c/L/n 行会归到上一个进程名下（端口/用户张冠李戴）。
    #[test]
    fn lsof_bad_pid_line_does_not_pollute_previous_process() {
        let text = "p123\ncnode\nn*:5173\npGARBAGE\ncpostgres\nn127.0.0.1:5432\n";
        let ls = parse_lsof(text);
        assert_eq!(ls.len(), 1, "坏 p 行之后的字段必须被丢弃");
        assert_eq!(ls[0].pid, 123);
        assert_eq!(ls[0].command, "node", "不得被后续 c 行覆盖");
        assert_eq!(ls[0].ports, vec![5173], "不得吸入后续 n 行端口");
    }

    #[test]
    fn cwd_parse_and_bad_pid_containment() {
        let text = "p100\nfcwd\nn/Users/x/proj\npBAD\nn/should/be/dropped\np200\nfcwd\nn/srv/app\n";
        let map = parse_cwd(text);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&100).unwrap(), "/Users/x/proj");
        assert_eq!(map.get(&200).unwrap(), "/srv/app");
    }

    /// 存活性证据的解析（KNOWN-GAPS Gap 1/A2）：只取 `->` **左侧**的本地端口。
    /// 取错方向的后果是实打实的漏报：一个正在抓网页的残留浏览器有大量出站连接，
    /// 按对端端口统计会让它永远「看起来有人在用」。
    #[test]
    fn established_parses_local_side_only() {
        let text = "\
p397
n127.0.0.1:9222->127.0.0.1:54191
n192.168.1.10:54999->93.184.216.34:443
p400
n[::1]:9333->[::1]:60123
";
        let map = parse_established(text);
        assert_eq!(map.get(&397).unwrap(), &vec![9222, 54999], "取本地端端口");
        assert_eq!(map.get(&400).unwrap(), &vec![9333], "IPv6 形态同样取本地端");
    }

    /// 监听行（无 `->`）不是连接，不得被算成存活证据；坏 p 行必须清空当前 PID，
    /// 否则后续 n 行会把别人的连接算到上一个进程头上（与 parse_lsof 同一底线）。
    #[test]
    fn established_ignores_listen_rows_and_contains_bad_pid() {
        let text = "p100\nn*:9222\nn127.0.0.1:9222->127.0.0.1:5000\npBAD\nn127.0.0.1:8888->127.0.0.1:6000\n";
        let map = parse_established(text);
        assert_eq!(map.get(&100).unwrap(), &vec![9222]);
        assert_eq!(map.len(), 1, "坏 p 行之后的连接必须被丢弃");
    }

    /// 生产真实形态的 ps 行（无 sess 列；state 的 's' 标志 = 会话首进程，
    /// 与真机 `ps -ax -o stat=,tty=` 输出一致）。
    #[test]
    fn ps_parse_realistic_rows_spaced_exe_and_dead_session() {
        let mut comm = HashMap::new();
        comm.insert(
            200u32,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron".to_string(),
        );
        // pid ppid state tty etime pcpu rss user command...
        let text = "\
  100     1 Ss   ??       01:00:00  0.0   1024 _postgres /opt/homebrew/opt/postgresql@16/bin/postgres -D /opt/homebrew/var
  200   150 S    ttys003  00:10:00  1.5  20480 fhf /Applications/Visual Studio Code.app/Contents/MacOS/Electron --type=utility
  300   200 S+   ttys007  00:05:00  0.2   5120 fhf node /Users/x/proj/node_modules/.bin/vite
";
        let procs = parse_ps(text, &comm, 1_000_000);
        let pg = procs.get(&100).unwrap();
        assert_eq!(pg.ppid, 1);
        assert_eq!(pg.elapsed_secs, 3600);
        assert_eq!(pg.start_unix, Some(1_000_000 - 3600));
        // user 列补齐无端口孤儿行（监听者仍优先 lsof L 字段）
        assert_eq!(pg.user, "_postgres");
        assert_eq!(procs.get(&300).unwrap().user, "fhf");
        assert_eq!(
            procs.get(&300).unwrap().full_command,
            "node /Users/x/proj/node_modules/.bin/vite"
        );
        // postgres 在 ?? 上：tty 信号永远中性
        assert!(!pg.tty_orphaned);
        // comm 修复空格路径
        let code = procs.get(&200).unwrap();
        assert_eq!(
            code.exe_path,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
        );
        // ttys003 / ttys007 上没有任何 state 含 's' 的进程 ⇒ 会话已死
        assert!(code.tty_orphaned);
        assert!(procs.get(&300).unwrap().tty_orphaned);
    }

    /// 回归（评审捕获的真机 bug）：健康终端 —— 登录 zsh 是 "Ss" 会话首进程，
    /// 同 tty 的 dev server 绝不能被标 tty_orphaned。
    #[test]
    fn ps_healthy_terminal_never_tty_orphaned() {
        let comm = HashMap::new();
        let text = "\
  500     1 Ss   ttys003  01:00:00  0.0   1024 fhf -zsh
  501   500 S+   ttys003  00:30:00  0.1   2048 fhf node server.js
  600     1 Ss+  ttys000  02:00:00  0.0   1024 fhf -zsh
";
        let procs = parse_ps(text, &comm, 1_000_000);
        assert!(!procs.get(&500).unwrap().tty_orphaned);
        assert!(
            !procs.get(&501).unwrap().tty_orphaned,
            "活终端里的 dev server 不能误报会话死"
        );
        assert!(!procs.get(&600).unwrap().tty_orphaned);
    }

    #[test]
    fn launchctl_pid_extraction() {
        let text = "PID\tStatus\tLabel\n123\t0\tcom.example.agent\n-\t0\tcom.example.idle\n77\t0\thomebrew.mxcl.postgresql@16\n";
        let pids = parse_launchctl(text);
        assert!(pids.contains(&123));
        assert!(pids.contains(&77));
        assert_eq!(pids.len(), 2);
    }

    #[test]
    fn identify_ladder() {
        // .app bundle（含空格路径）
        let (label, cat) = identify_app(
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron --type=utility",
            "Electron",
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
        );
        assert_eq!(label, "Visual Studio Code");
        assert_eq!(cat, "installed-app");

        // node_modules 下的 Electron.app（electron / electron-vite 的 dev runtime）：
        // 形态与 /Applications 的真应用相同，但必须归 dev-script 才不会被 installed-app
        // 豁免吞掉 —— 否则孤儿 Electron（dev 残留）永远检测不到。
        let (label, cat) = identify_app(
            "/Users/x/proj/node_modules/.pnpm/electron@33.4.11/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron .",
            "Electron",
            "/Users/x/proj/node_modules/.pnpm/electron@33.4.11/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron",
        );
        assert_eq!(label, "Electron");
        assert_eq!(cat, "dev-script");

        // 系统组件
        let (label, cat) = identify_app("/usr/sbin/cupsd -l", "cupsd", "/usr/sbin/cupsd");
        assert_eq!(label, "cupsd");
        assert_eq!(cat, "system");

        // 脚本运行时 + 项目提取
        let (label, cat) = identify_app(
            "node /Users/x/proj/node_modules/vite/bin/vite.js",
            "node",
            "/usr/local/bin/node",
        );
        assert_eq!(label, "proj · vite.js");
        assert_eq!(cat, "dev-script");

        // Homebrew CLI
        let (label, cat) = identify_app(
            "/opt/homebrew/bin/redis-server *:6379",
            "redis-server",
            "/opt/homebrew/bin/redis-server",
        );
        assert_eq!(label, "redis-server");
        assert_eq!(cat, "user-binary");

        // Cargo 产物
        let (_, cat) = identify_app(
            "/Users/x/rust/mytool/target/debug/mytool",
            "mytool",
            "/Users/x/rust/mytool/target/debug/mytool",
        );
        assert_eq!(cat, "dev-script");

        // 回归（评审发现的真实漏报）：go run 临时编译产物在 /private/var/folders
        // 下，必须拿到 dev-script 身份 —— 否则被标准路径前缀整体豁免，
        // 孤儿 go run 服务永远不可见（CLAUDE.md 明示 cargo run 同类是产品目标）
        let (label, cat) = identify_app(
            "/private/var/folders/dx/T/go-build123/b001/exe/server --port 8080",
            "server",
            "/private/var/folders/dx/T/go-build123/b001/exe/server",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "server");

        // `-m 模块` 调用：身份是模块，不因解释器在 brew/系统路径而改类。
        // 必须用完整真实路径：brew 的 Python 住在 Cellar 的 Python.app bundle
        // 里，曾被 .app 阶梯先一步归 installed-app 豁免（真实漏报案例：
        // 孤儿 python -m http.server 占着 8000 端口）。
        let (label, cat) = identify_app(
            "/opt/homebrew/Cellar/python@3.14/3.14.5/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python -m http.server 8000",
            "Python",
            "/opt/homebrew/Cellar/python@3.14/3.14.5/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python",
        );
        assert_eq!(label, "http.server · Python");
        assert_eq!(cat, "dev-script");

        // 同形态跑用户脚本：身份是脚本，.app 包装不豁免
        let (label, cat) = identify_app(
            "/Library/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python /Users/x/bot/main.py",
            "Python",
            "/Library/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python",
        );
        assert_eq!(label, "main.py · Python");
        assert_eq!(cat, "dev-script");

        let (label, cat) = identify_app(
            "/usr/bin/python3 -m http.server 9000",
            "python3",
            "/usr/bin/python3",
        );
        assert_eq!(label, "http.server · python3");
        assert_eq!(cat, "dev-script");

        // KNOWN-GAPS Gap 1：headless 自动化实例的身份在命令行 —— 必须先于
        // .app / /Applications 阶梯判定，否则归 installed-app 即吃硬豁免。
        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let (label, cat) = identify_app(
            &format!(
                "{CHROME} --headless=new --user-data-dir=/private/tmp/sess/prof \
                 --remote-debugging-port=9339 about:blank"
            ),
            "Google Chrome",
            CHROME,
        );
        assert_eq!(label, "Google Chrome · headless");
        assert_eq!(cat, super::super::AUTOMATION_CATEGORY);

        // 对照：同一个 exe，无自动化开关 ⇒ 仍是用户日常那个 Chrome
        let (label, cat) = identify_app(CHROME, "Google Chrome", CHROME);
        assert_eq!(label, "Google Chrome");
        assert_eq!(cat, "installed-app");

        // Playwright 下载到 Caches 的 Chromium.app：形态同真应用，但归 dev-script
        //（与 node_modules 下的 Electron.app 同一条不变量）
        let pw = "/Users/x/Library/Caches/ms-playwright/chromium-1148/chrome-mac/Chromium.app/Contents/MacOS/Chromium";
        let (label, cat) = identify_app(pw, "Chromium", pw);
        assert_eq!(label, "Chromium");
        assert_eq!(cat, "dev-script");
    }

    /// 豁免谓词与事实谓词必须在 App Translocation 目录上**给出相反答案** ——
    /// 这正是评审捕获的坑：拿豁免谓词陈述事实，`go run` 的临时编译产物
    ///（真住在 /private/var/folders/，identify_app 归 dev-script）会被断言成
    /// 「装在标准位置」，从而丢掉 NonstandardPath —— 而那恰是这条理由最成立的场景。
    #[test]
    fn conventional_path_excludes_the_translocation_carveout() {
        const GO_RUN: &str = "/private/var/folders/dx/T/go-build123/b001/exe/server";
        assert!(
            is_standard_install_path(GO_RUN),
            "豁免侧必须继续放行（App Translocation 的既有语义不动）"
        );
        assert!(
            !is_conventional_install_path(GO_RUN),
            "事实侧必须说实话：临时目录不是常规安装位置"
        );

        // 两者一致的部分：真正的安装位置
        for p in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/usr/bin/python3",
            "/System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter",
        ] {
            assert!(is_standard_install_path(p), "{p}");
            assert!(is_conventional_install_path(p), "{p}");
        }
        // 两者一致的部分：确实非标准（brew、用户目录、读不到 exe）
        for p in ["/opt/homebrew/bin/node", "/Users/x/.vite-plus/node", ""] {
            assert!(!is_standard_install_path(p), "{p}");
            assert!(!is_conventional_install_path(p), "{p}");
        }
    }

    #[test]
    fn brew_service_detection() {
        assert!(is_brew_service_path(
            "/opt/homebrew/opt/postgresql@16/bin/postgres"
        ));
        assert!(is_brew_service_path(
            "/usr/local/Cellar/redis/7.2/bin/redis-server"
        ));
        assert!(!is_brew_service_path("/opt/homebrew/bin/node"));
        assert!(!is_brew_service_path("/Users/x/bin/server"));
    }

    #[test]
    fn shell_detection_handles_login_dash() {
        assert!(is_shell("/bin/zsh"));
        assert!(is_shell("-zsh"));
        assert!(is_shell("/opt/homebrew/bin/fish"));
        assert!(!is_shell("/usr/local/bin/node"));
    }
}
