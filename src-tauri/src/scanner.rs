use serde::Serialize;
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Serialize, Clone)]
pub struct ParentRef {
    pub pid: u32,
    pub label: String,
    pub category: String,
    pub exe_path: String,
}

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
    pub tty: String,
    pub elapsed: String,
    pub cpu_percent: f32,
    pub mem_mb: f32,
    pub state: String,
    pub is_zombie_suspect: bool,
    pub zombie_reasons: Vec<String>,
    pub is_whitelisted: bool,
}

struct LsofEntry {
    pid: u32,
    command: String,
    user: String,
    ports: Vec<u16>,
}

struct PsEntry {
    ppid: u32,
    state: String,
    tty: String,
    elapsed: String,
    cpu_percent: f32,
    rss_kb: u64,
    command: String,
}

const DEV_SERVER_PATTERNS: &[&str] = &[
    "node", "vite", "next", "nest", "remix", "nuxt", "astro", "svelte",
    "python", "uvicorn", "gunicorn", "flask", "fastapi", "django", "hypercorn",
    "ruby", "rails", "puma", "unicorn",
    "deno", "bun", "tsx", "ts-node", "esbuild", "webpack", "rspack", "turbopack",
    "rollup", "parcel", "snowpack",
    "tauri", "electron",
    "java", "tomcat", "jetty",
    "go run", "air ",
    "cargo run", "cargo-watch",
    "php", "artisan",
    "http.server", "live-server", "browser-sync", "serve", "http-server",
    "ngrok", "cloudflared",
    "streamlit", "gradio", "jupyter",
];

// 标准安装路径前缀 —— 这些位置的可执行文件视为「正规 app / 系统组件」
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

const SCRIPT_RUNTIMES: &[&str] = &[
    "node", "python", "python3", "ruby", "java", "bun", "deno", "php", "perl",
];

fn is_dev_server(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    DEV_SERVER_PATTERNS.iter().any(|p| lower.contains(p))
}

fn extract_exe_path(full_cmd: &str) -> &str {
    full_cmd.split_whitespace().next().unwrap_or("")
}

fn is_system_app(exe_path: &str) -> bool {
    SYSTEM_PATH_PREFIXES.iter().any(|p| exe_path.starts_with(p))
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// 从 path 推断「项目名」—— /Users/fhf/IT/code/portreaper/node_modules/... → "portreaper"
fn extract_project_name(path: &str) -> Option<String> {
    let after_users = path.split("/Users/").nth(1)?;
    let segments: Vec<&str> = after_users.split('/').collect();
    if segments.len() < 3 {
        return None;
    }
    let path_segments = &segments[1..];
    let stop_words = [
        "node_modules", "target", "src", "src-tauri", ".bin",
        "dist", "build", ".venv", "venv", ".next", ".nuxt", "out",
    ];
    for (i, s) in path_segments.iter().enumerate() {
        if stop_words.contains(s) && i > 0 {
            return Some(path_segments[i - 1].to_string());
        }
    }
    None
}

/// (label, category)
fn identify_app(full_command: &str, short_command: &str) -> (String, String) {
    let exe = extract_exe_path(full_command);

    // 1. macOS .app bundle —— 抽出 .app 名
    if let Some(idx) = exe.find(".app/") {
        let before = &exe[..idx]; // 不含 ".app"
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

    // 2a. /Applications/ 下的裸二进制（没有 .app 包结构，老式 app 或 Clash 这类）
    if exe.starts_with("/Applications/") {
        return (basename(exe).to_string(), "installed-app".to_string());
    }

    // 2b. 系统组件
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

    // 3. 脚本运行时 (node / python / java …) —— 抽出脚本名 + 项目名
    let runtime_lower = short_command.to_lowercase();
    if SCRIPT_RUNTIMES.contains(&runtime_lower.as_str()) {
        let parts: Vec<&str> = full_command.split_whitespace().collect();
        let script = parts.iter().skip(1).find(|a| {
            a.ends_with(".js") || a.ends_with(".mjs") || a.ends_with(".ts")
                || a.ends_with(".cjs") || a.ends_with(".py") || a.ends_with(".rb")
                || a.ends_with(".jar") || a.ends_with(".php")
        });

        let project = extract_project_name(full_command);

        // 输出格式: 「主标识 · 副标识」，前端会按 " · " 拆成两行渲染（主粗体，副灰小字）
        // 顺序：能识别出项目就把项目当主标识；否则脚本/命令当主
        let label = match (script, project) {
            (Some(s), Some(p)) => format!("{} · {}", p, basename(s)),
            (Some(s), None) => format!("{} · {}", basename(s), short_command),
            (None, Some(p)) => format!("{} · {}", p, short_command),
            (None, None) => short_command.to_string(),
        };
        return (label, "dev-script".to_string());
    }

    // 4. /usr/local/bin/, /opt/homebrew/{bin,opt}/ → 用户安装的 CLI
    if exe.starts_with("/usr/local/")
        || exe.starts_with("/opt/homebrew/")
        || exe.starts_with("/opt/local/")
    {
        return (basename(exe).to_string(), "user-binary".to_string());
    }

    // 5. /target/{debug,release}/ → Rust / Cargo 产物
    if exe.contains("/target/debug/") || exe.contains("/target/release/") {
        let bin = basename(exe);
        let label = match extract_project_name(exe) {
            Some(p) if p != bin => format!("{} · {}", p, bin),
            _ => bin.to_string(),
        };
        return (label, "dev-script".to_string());
    }

    // 6. /Users/... 但不在上面任何位置 → 用户目录下的自定义二进制
    if exe.starts_with("/Users/") {
        let bin = basename(exe);
        let label = match extract_project_name(exe) {
            Some(p) if p != bin => format!("{} · {}", p, bin),
            _ => bin.to_string(),
        };
        return (label, "dev-script".to_string());
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

fn parse_lsof() -> Vec<LsofEntry> {
    let output = match Command::new("lsof")
        .args(["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-FpcLn"])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
    {
        Ok(o) => o,
        Err(_) => return vec![],
    };
    let text = String::from_utf8_lossy(&output.stdout);

    let mut by_pid: HashMap<u32, LsofEntry> = HashMap::new();
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
                    by_pid.entry(pid).or_insert(LsofEntry {
                        pid,
                        command: String::new(),
                        user: String::new(),
                        ports: vec![],
                    });
                }
            }
            b'c' => {
                if let Some(pid) = current_pid {
                    if let Some(e) = by_pid.get_mut(&pid) {
                        e.command = rest.to_string();
                    }
                }
            }
            b'L' => {
                if let Some(pid) = current_pid {
                    if let Some(e) = by_pid.get_mut(&pid) {
                        e.user = rest.to_string();
                    }
                }
            }
            b'n' => {
                let addr = rest.split("->").next().unwrap_or(rest);
                if let Some(port_str) = addr.rsplit(':').next() {
                    if let Ok(port) = port_str.parse::<u16>() {
                        if let Some(pid) = current_pid {
                            if let Some(e) = by_pid.get_mut(&pid) {
                                if !e.ports.contains(&port) {
                                    e.ports.push(port);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    by_pid.into_values().collect()
}

fn parse_ps() -> HashMap<u32, PsEntry> {
    let output = match Command::new("ps")
        .args([
            "-A",
            "-o",
            "pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,command=",
        ])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
    {
        Ok(o) => o,
        Err(_) => return HashMap::new(),
    };
    let text = String::from_utf8_lossy(&output.stdout);
    let mut map = HashMap::new();

    for line in text.lines() {
        let line = line.trim_start();
        let mut iter = line.split_whitespace();
        let pid: u32 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let ppid: u32 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let state = match iter.next() {
            Some(v) => v.to_string(),
            None => continue,
        };
        let tty = match iter.next() {
            Some(v) => v.to_string(),
            None => continue,
        };
        let elapsed = match iter.next() {
            Some(v) => v.to_string(),
            None => continue,
        };
        let pcpu: f32 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let rss_kb: u64 = match iter.next().and_then(|s| s.parse().ok()) {
            Some(v) => v,
            None => continue,
        };
        let command: String = iter.collect::<Vec<_>>().join(" ");
        map.insert(
            pid,
            PsEntry {
                ppid,
                state,
                tty,
                elapsed,
                cpu_percent: pcpu,
                rss_kb,
                command,
            },
        );
    }
    map
}

fn classify(
    ppid: u32,
    state: &str,
    full_command: &str,
    short_command: &str,
    exe_path: &str,
    app_category: &str,
) -> (bool, Vec<String>) {
    let mut reasons = Vec::new();

    // 真正的 defunct zombie —— 永远标记
    if state.contains('Z') {
        reasons.push("进程已死 (defunct)".into());
        return (true, reasons);
    }

    // 父进程还活着 —— 不是僵尸
    if ppid != 1 {
        return (false, reasons);
    }

    // 路径在标准安装位置 —— 是正规 macOS app / 系统组件
    if is_system_app(exe_path) || app_category == "installed-app" || app_category == "system" {
        return (false, reasons);
    }

    // 走到这里：PPID=1 且 路径不在标准安装位置 → 强信号
    reasons.push("PPID=1 (孤儿)".into());
    reasons.push("非标准安装路径".into());

    if is_dev_server(full_command) || is_dev_server(short_command) {
        reasons.push("dev-server 关键字".into());
    }

    (true, reasons)
}

fn build_parent_chain(
    start_pid: u32,
    ps_map: &HashMap<u32, PsEntry>,
) -> Vec<ParentRef> {
    let mut chain = Vec::new();
    let mut current_pid = start_pid;
    for _ in 0..12 {
        let parent_ppid = match ps_map.get(&current_pid) {
            Some(p) => p.ppid,
            None => break,
        };
        if parent_ppid == 0 || parent_ppid == current_pid {
            break;
        }
        if parent_ppid == 1 {
            chain.push(ParentRef {
                pid: 1,
                label: "launchd".to_string(),
                category: "system".to_string(),
                exe_path: "/sbin/launchd".to_string(),
            });
            break;
        }
        let parent_entry = match ps_map.get(&parent_ppid) {
            Some(p) => p,
            None => break,
        };
        let parent_exe = extract_exe_path(&parent_entry.command).to_string();
        let parent_basename = basename(&parent_exe).to_string();
        let (label, category) = identify_app(&parent_entry.command, &parent_basename);
        let is_user_visible_app = category == "installed-app";
        chain.push(ParentRef {
            pid: parent_ppid,
            label,
            category,
            exe_path: parent_exe,
        });
        if is_user_visible_app {
            break;
        }
        current_pid = parent_ppid;
    }
    chain
}

pub fn scan(whitelist: &[String]) -> Vec<ProcessEntry> {
    let lsof = parse_lsof();
    let ps_map = parse_ps();

    let mut entries = Vec::new();
    for l in lsof {
        let ps = ps_map.get(&l.pid);
        let ppid = ps.map(|p| p.ppid).unwrap_or(0);
        let tty = ps.map(|p| p.tty.clone()).unwrap_or_else(|| "?".into());
        let elapsed = ps.map(|p| p.elapsed.clone()).unwrap_or_default();
        let pcpu = ps.map(|p| p.cpu_percent).unwrap_or(0.0);
        let mem_mb = ps.map(|p| p.rss_kb as f32 / 1024.0).unwrap_or(0.0);
        let state = ps.map(|p| p.state.clone()).unwrap_or_default();
        let full_command = ps
            .map(|p| p.command.clone())
            .unwrap_or_else(|| l.command.clone());
        let exe_path = extract_exe_path(&full_command).to_string();

        let (app_label, app_category) = identify_app(&full_command, &l.command);
        let parent_chain = build_parent_chain(l.pid, &ps_map);
        let launcher_label = parent_chain
            .last()
            .map(|p| p.label.clone())
            .unwrap_or_else(|| "?".to_string());

        let (suspect, reasons) = classify(
            ppid,
            &state,
            &full_command,
            &l.command,
            &exe_path,
            &app_category,
        );

        // 白名单 key: 优先用 exe_path（最稳定），否则退回 command
        let wl_key = if !exe_path.is_empty() {
            exe_path.clone()
        } else {
            l.command.clone()
        };
        let is_whitelisted = whitelist.contains(&wl_key);

        let mut ports = l.ports.clone();
        ports.sort_unstable();

        entries.push(ProcessEntry {
            pid: l.pid,
            ppid,
            ports,
            command: l.command.clone(),
            full_command,
            exe_path,
            app_label,
            app_category,
            parent_chain,
            launcher_label,
            user: l.user.clone(),
            tty,
            elapsed,
            cpu_percent: pcpu,
            mem_mb,
            state,
            is_zombie_suspect: suspect && !is_whitelisted,
            zombie_reasons: reasons,
            is_whitelisted,
        });
    }

    entries.sort_by(|a, b| {
        b.is_zombie_suspect
            .cmp(&a.is_zombie_suspect)
            .then(a.ports.first().copied().unwrap_or(0).cmp(&b.ports.first().copied().unwrap_or(0)))
    });

    entries
}
