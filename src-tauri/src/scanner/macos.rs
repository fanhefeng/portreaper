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

// 标准安装路径前缀 —— 这些位置的可执行文件视为「正规 app / 系统组件」，永不自动标记
const SYSTEM_PATH_PREFIXES: &[&str] = &[
    "/Applications/",
    "/System/",
    "/Library/",
    "/usr/libexec/",
    "/usr/sbin/",
    "/usr/bin/",
    "/sbin/",
    "/bin/",
    "/private/var/folders/",
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
    SYSTEM_PATH_PREFIXES.iter().any(|p| exe_path.starts_with(p))
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
                format!("{} · {}", module, short_command.to_lowercase()),
                "dev-script".to_string(),
            );
        }
    }

    // 1. .app bundle —— 抽出 .app 名（exe 来自 ps comm，含空格也完整）
    if let Some(idx) = exe.find(".app/") {
        let before = &exe[..idx];
        if let Some(slash) = before.rfind('/') {
            let app_name = &before[slash + 1..];
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

    // 2c. 系统组件
    if exe.starts_with("/System/")
        || exe.starts_with("/Library/")
        || exe.starts_with("/usr/libexec/")
        || exe.starts_with("/usr/sbin/")
        || exe.starts_with("/sbin/")
        || exe.starts_with("/usr/bin/")
        || exe.starts_with("/bin/")
    {
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

    // 5. /target/{debug,release}/ → Rust / Cargo 产物；`go run` 的临时编译产物
    //    （/private/var/folders/.../go-build*/exe/main）同理是 dev 进程 ——
    //    必须先于路径豁免给出 dev-script 身份，否则 /private/var/folders/ 的
    //    标准路径前缀会把孤儿 go run 服务整体豁免（评审发现的真实漏报；该前缀
    //    本为 App Translocation 设，而那些路径含 .app/ 早被阶梯 1 接住）。
    if exe.contains("/target/debug/")
        || exe.contains("/target/release/")
        || exe.contains("/go-build")
    {
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

fn cmd_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(crate) fn collect() -> Collected {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let listeners = parse_lsof(
        &cmd_output("lsof", &["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-FpcLn"]).unwrap_or_default(),
    );
    let comm_map = parse_comm(&cmd_output("ps", &["-A", "-o", "pid=,comm="]).unwrap_or_default());
    let procs = parse_ps(
        &cmd_output(
            "ps",
            &[
                "-A",
                "-o",
                "pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,command=",
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
            &cmd_output("lsof", &["-a", "-p", &pid_csv, "-d", "cwd", "-Fpn"]).unwrap_or_default(),
        )
    };

    Collected {
        listeners,
        procs,
        launchd_pids,
        cwds,
    }
}

/// `lsof -a -p <pids> -d cwd -Fpn` 输出：pPID 行后跟 n<路径> 行。
fn parse_cwd(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
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
        let rest = &line[1..];
        match tag {
            b'p' => {
                if let Ok(pid) = rest.parse::<u32>() {
                    current_pid = Some(pid);
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

/// 解析 [[dd-]hh:]mm:ss 形式的 etime 为秒。
pub(crate) fn parse_etime(s: &str) -> u64 {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().unwrap_or(0), r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse::<u64>().unwrap_or(0))
        .collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        [s] => (0, 0, *s),
        _ => (0, 0, 0),
    };
    days * 86_400 + h * 3_600 + m * 60 + sec
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
                user: String::new(), // 监听者的 user 来自 lsof L 字段
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

    /// 生产真实形态的 ps 行（无 sess 列；state 的 's' 标志 = 会话首进程，
    /// 与真机 `ps -ax -o stat=,tty=` 输出一致）。
    #[test]
    fn ps_parse_realistic_rows_spaced_exe_and_dead_session() {
        let mut comm = HashMap::new();
        comm.insert(
            200u32,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron".to_string(),
        );
        // pid ppid state tty etime pcpu rss command...
        let text = "\
  100     1 Ss   ??       01:00:00  0.0   1024 /opt/homebrew/opt/postgresql@16/bin/postgres -D /opt/homebrew/var
  200   150 S    ttys003  00:10:00  1.5  20480 /Applications/Visual Studio Code.app/Contents/MacOS/Electron --type=utility
  300   200 S+   ttys007  00:05:00  0.2   5120 node /Users/x/proj/node_modules/.bin/vite
";
        let procs = parse_ps(text, &comm, 1_000_000);
        let pg = procs.get(&100).unwrap();
        assert_eq!(pg.ppid, 1);
        assert_eq!(pg.elapsed_secs, 3600);
        assert_eq!(pg.start_unix, Some(1_000_000 - 3600));
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
  500     1 Ss   ttys003  01:00:00  0.0   1024 -zsh
  501   500 S+   ttys003  00:30:00  0.1   2048 node server.js
  600     1 Ss+  ttys000  02:00:00  0.0   1024 -zsh
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
        assert_eq!(label, "http.server · python");
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
