//! Portreaper 的命令行前端 —— 判定引擎的**进程边界**。
//!
//! 存在意义：让桌面版之外的前端（Raycast 扩展、shell 脚本、CI 检查）复用同一套
//! 孤儿进程判定，而不是各自重写一份。判定逻辑一行都不在这里 —— 本文件只做
//! 「参数解析 → 调 portreaper_core → 序列化」。
//!
//! # 契约
//!
//! ```text
//! portreaper-cli scan [--json] [--no-orphans] [--cpu=skip|<ms>]
//! portreaper-cli kill <pid> --start-unix <n> [--force]
//! portreaper-cli whitelist list|add <key>|remove <key>
//! portreaper-cli --version | --help
//! ```
//!
//! `--json` 输出带 `schemaVersion`：消费方读到不认识的**主**版本应当提示升级，
//! 而不是照着渲染出错乱的行。新增可选字段不递增主版本。
//!
//! # kill 的安全性是白送的
//!
//! `kill` **强制**要求 `--start-unix`（扫描时捕获的进程创建时间）。引擎的身份
//! 校验是 fail-closed 的：没有令牌直接拒绝。于是任何调用方都必须「先 scan 再
//! kill」，PID 复用防护自动覆盖到所有前端 —— 这是既有设计白送的红利，**不要**
//! 加 `--yes` 之类的旁路把它绕过去。

use std::io::Write;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use portreaper_core::{scanner, CpuSampling, Whitelist};
use serde::Serialize;

/// JSON 输出的契约版本。**破坏性变更**（删字段、改字段语义）才递增；
/// 新增可选字段不动它。消费方见到更大的主版本应当停下并提示升级。
const SCHEMA_VERSION: u32 = 1;

const USAGE: &str = "\
portreaper-cli — 找出并终止无人认领的开发进程（Portreaper 的命令行前端）

用法:
  portreaper-cli scan [--json] [--no-orphans] [--cpu=skip|<毫秒>]
  portreaper-cli kill <pid> --start-unix <创建时间> [--force]
  portreaper-cli whitelist list
  portreaper-cli whitelist add <key>
  portreaper-cli whitelist remove <key>
  portreaper-cli --version | --help

scan 选项:
  --json           输出 JSON（供 Raycast / 脚本消费），否则输出人类可读表格
  --no-orphans     只列监听端口的进程，跳过不占端口的孤儿 dev 进程
  --cpu=skip       跳过 CPU 采样预热（最快；Windows 上 CPU 一律显示 0%）
  --cpu=<毫秒>     预热后等待指定毫秒再采集（默认 200）

kill 选项:
  --start-unix <n> 扫描时该行的 start_unix。**必填** —— 引擎据此核对进程身份，
                   防止 scan 与 kill 之间 PID 被复用导致误杀。先 scan 拿到它。
  --force          macOS 用 SIGKILL 而非 SIGTERM；Windows 无效（只有一种终止方式）

退出码:
  0  成功
  1  失败（kill 的失败原因以 JSON 写到 stderr，形如 {\"code\":\"pid_reused\"}）
  2  用法错误
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&args);
    std::process::exit(code);
}

fn run(args: &[String]) -> i32 {
    let Some(first) = args.first().map(String::as_str) else {
        eprint!("{USAGE}");
        return 2;
    };
    match first {
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            0
        }
        "--version" | "-V" => {
            println!("portreaper-cli {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "scan" => cmd_scan(&args[1..]),
        "kill" => cmd_kill(&args[1..]),
        "whitelist" => cmd_whitelist(&args[1..]),
        other => {
            eprintln!("未知子命令: {other}\n");
            eprint!("{USAGE}");
            2
        }
    }
}

// ---------------------------------------------------------------------------
// scan
// ---------------------------------------------------------------------------

/// 全字段 snake_case —— **刻意不加 `rename_all = "camelCase"`**。
///
/// `entries` 里的 `ProcessEntry` 是引擎的 serde 输出，用的是 Rust 默认的
/// snake_case（`is_zombie_suspect` / `app_label` …），桌面前端的
/// `src/model.ts` 镜像的正是它。外层若擅自用 camelCase，同一份 JSON 里就会出现
/// 两种命名风格，消费方每取一个字段都要先想「这层是哪种」—— 实测中一眼看出的
/// 不一致。整体统一 snake_case 的额外好处：Raycast 扩展可以直接复用
/// `src/model.ts` 的 `ProcessEntry` 类型，一个字段都不用改。
#[derive(Serialize)]
struct ScanReport<'a> {
    schema_version: u32,
    scanned_at: u64,
    platform: &'static str,
    entries: &'a [scanner::ProcessEntry],
}

fn cmd_scan(args: &[String]) -> i32 {
    let mut json = false;
    let mut include_orphans = true;
    let mut cpu = CpuSampling::default();

    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "--no-orphans" => include_orphans = false,
            "--cpu=skip" => cpu = CpuSampling::Skip,
            _ if a.starts_with("--cpu=") => {
                let raw = &a["--cpu=".len()..];
                match raw.parse::<u64>() {
                    Ok(ms) => cpu = CpuSampling::Interval(Duration::from_millis(ms)),
                    Err(_) => {
                        eprintln!("--cpu 需要 `skip` 或毫秒数，收到: {raw}");
                        return 2;
                    }
                }
            }
            _ => {
                eprintln!("scan: 无法识别的参数 {a}\n");
                eprint!("{USAGE}");
                return 2;
            }
        }
    }

    let whitelist = load_whitelist();
    let mut entries = scanner::scan_once(whitelist.entries(), cpu);
    if !include_orphans {
        // 无端口的行来自第二条扫描路径（孤儿 dev 进程）
        entries.retain(|e| !e.ports.is_empty());
    }

    if json {
        let report = ScanReport {
            schema_version: SCHEMA_VERSION,
            scanned_at: now_unix(),
            platform: platform_name(),
            entries: &entries,
        };
        match serde_json::to_string(&report) {
            Ok(s) => {
                println!("{s}");
                0
            }
            Err(e) => {
                eprintln!("序列化失败: {e}");
                1
            }
        }
    } else {
        print_table(&entries);
        0
    }
}

/// 人类可读输出。刻意保持朴素：这是给「在终端里随手看一眼」用的，
/// 需要稳定结构的消费方一律走 `--json`。
fn print_table(entries: &[scanner::ProcessEntry]) {
    if entries.is_empty() {
        println!("没有发现监听端口的进程。");
        return;
    }
    let suspects = entries.iter().filter(|e| e.is_zombie_suspect).count();
    println!(
        "{} 行，{} 个疑似残留（★ = 已收藏，豁免判定）\n",
        entries.len(),
        suspects
    );
    println!(
        "{:<7} {:<12} {:<10} {:<9} {:>7}  进程",
        "PID", "端口", "置信度", "类别", "CPU%"
    );
    for e in entries {
        let ports = format_ports(&e.ports);
        let verdict = if e.is_whitelisted {
            "★".to_string()
        } else if e.is_zombie_suspect {
            format!("{:?}", e.confidence).to_lowercase()
        } else {
            "-".to_string()
        };
        println!(
            "{:<7} {:<12} {:<10} {:<9} {:>7.1}  {}",
            e.pid, ports, verdict, e.app_category, e.cpu_percent_tree, e.app_label
        );
    }
}

/// 端口列的显示形态：列宽固定，多端口进程（微信能开五个）必须压缩，
/// 否则一行撑开会把后面所有列的对齐一起带歪。完整列表走 `--json`。
fn format_ports(ports: &[u16]) -> String {
    match ports {
        [] => "—".to_string(), // 无端口的孤儿 dev 进程
        [only] => only.to_string(),
        [first, rest @ ..] => format!("{first},+{}", rest.len()),
    }
}

// ---------------------------------------------------------------------------
// kill
// ---------------------------------------------------------------------------

fn cmd_kill(args: &[String]) -> i32 {
    let mut pid: Option<u32> = None;
    let mut start_unix: Option<u64> = None;
    let mut force = false;

    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--force" | "-9" => force = true,
            "--start-unix" => match it.next().map(|v| v.parse::<u64>()) {
                Some(Ok(v)) => start_unix = Some(v),
                _ => {
                    eprintln!("--start-unix 需要一个整数（进程创建时间，epoch 秒）");
                    return 2;
                }
            },
            _ if pid.is_none() => match a.parse::<u32>() {
                Ok(v) => pid = Some(v),
                Err(_) => {
                    eprintln!("kill: 无效的 PID {a}");
                    return 2;
                }
            },
            _ => {
                eprintln!("kill: 无法识别的参数 {a}");
                return 2;
            }
        }
    }

    let Some(pid) = pid else {
        eprintln!("kill: 缺少 PID\n");
        eprint!("{USAGE}");
        return 2;
    };
    if start_unix.is_none() {
        // 不把它当成「可选参数缺省」——引擎会 fail-closed 拒绝，但在这里给出
        // 更有用的解释：这个约束是防误杀的，不是形式主义。
        eprintln!(
            "kill: 缺少 --start-unix。\n\
             这是扫描时捕获的进程创建时间，用于在终止前核对进程身份 —— \n\
             没有它就无法区分「目标进程」和「PID 被回收后新起的另一个进程」。\n\
             先跑 `portreaper-cli scan --json`，取该行的 start_unix 传进来。"
        );
        return 2;
    }

    match portreaper_core::kill(pid, force, start_unix) {
        Ok(()) => 0,
        Err(e) => {
            // 结构化写到 stderr：调用方按 code 分支，不必解析人类文案
            let json = serde_json::to_string(&e).unwrap_or_else(|_| r#"{"code":"os"}"#.to_string());
            let mut err = std::io::stderr();
            let _ = writeln!(err, "{json}");
            let _ = writeln!(err, "kill {pid} 失败: {e}");
            1
        }
    }
}

// ---------------------------------------------------------------------------
// whitelist
// ---------------------------------------------------------------------------

fn cmd_whitelist(args: &[String]) -> i32 {
    let Some(sub) = args.first().map(String::as_str) else {
        eprintln!("whitelist: 需要 list | add <key> | remove <key>");
        return 2;
    };
    let mut wl = load_whitelist();
    match sub {
        "list" => {
            for e in wl.entries() {
                println!("{e}");
            }
            0
        }
        "add" | "remove" => {
            let Some(key) = args.get(1) else {
                eprintln!("whitelist {sub}: 缺少 key");
                return 2;
            };
            let res = if sub == "add" {
                wl.add(key.clone())
            } else {
                wl.remove(key)
            };
            match res {
                Ok(()) => 0,
                Err(e) => {
                    eprintln!("whitelist {sub} 失败: {e}");
                    1
                }
            }
        }
        other => {
            eprintln!("whitelist: 未知操作 {other}（可用 list | add | remove）");
            2
        }
    }
}

// ---------------------------------------------------------------------------
// 共用
// ---------------------------------------------------------------------------

/// 与桌面版共享同一个白名单文件 —— 路径由引擎给出，CLI 绝不自己拼。
/// 解析不到配置目录时降级为空白名单（扫描照常，只是星标不生效且存不下来）。
fn load_whitelist() -> Whitelist {
    match portreaper_core::paths::whitelist_path() {
        Some(p) => Whitelist::load(p),
        None => {
            eprintln!("警告: 无法解析配置目录，白名单本次不生效");
            Whitelist::empty(std::path::PathBuf::new())
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "unknown"
    }
}

#[cfg(test)]
mod tests {
    use super::format_ports;

    #[test]
    fn ports_column_stays_narrow() {
        assert_eq!(format_ports(&[]), "—");
        assert_eq!(format_ports(&[5173]), "5173");
        assert_eq!(
            format_ports(&[14013, 14016, 14019, 14022, 14023]),
            "14013,+4"
        );
        // 无论多少个端口，宽度都不超过「端口号 + ,+N」
        assert!(format_ports(&[65535; 9]).len() <= 8);
    }
}
