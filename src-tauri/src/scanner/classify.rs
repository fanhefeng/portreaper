use serde::Serialize;

use super::identify::{basename, strip_exe};
use super::model::ProcessSnapshot;

/// 僵尸判定原因 —— 机器码，serde 蛇形小写输出，前端按 `reason.<code>` 做 i18n。
/// ⚠️ 增删变体时必须同步 src/i18n.ts 的 reason.* 字典（CI 的 check-reason-parity.mjs 会校验）。
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReasonCode {
    // —— 正向信号（提升嫌疑）——
    /// 进程已死（ps state 含 Z）
    Defunct,
    /// macOS：被 launchd 收养（PPID=1），原启动者已退出
    #[cfg_attr(windows, allow(dead_code))]
    Ppid1Orphan,
    /// Windows：父进程已退出（PPID 不在进程表中）
    #[cfg_attr(not(windows), allow(dead_code))]
    ParentExited,
    /// Windows：父 PID 槽位已被更晚创建的进程复用 —— 真实父已死
    #[cfg_attr(not(windows), allow(dead_code))]
    PidSlotReused,
    /// 孤儿链：父链走到 init/死根且途中无任何存活的用户可见 App
    OrphanedChain,
    /// 有真实 tty 但其会话首进程已不在（终端崩溃/被杀）
    OrphanedSession,
    /// exe 不在标准安装路径
    NonstandardPath,
    /// 命令行命中 dev-server 关键字
    DevServerKeyword,
    /// 同项目重复 dev server（另一实例见 ProcessEntry::duplicate_of）。
    /// 由 scan() 的跨条目后处理标注（classify 是单进程纯函数，看不到其他条目）；
    /// 只到 Possible，永不入清扫 —— 机器无法判断用户正在用哪个实例。
    DuplicateDevServer,
    // —— 豁免/降级信号（解释为什么不标记 / 降级）——
    /// launchctl 认领的任务（LaunchAgent / brew services 等）
    LaunchdManaged,
    /// exe 位于 Homebrew 服务路径（launchctl 探不到 system-domain 时的兜底）
    BrewServicePath,
    /// 标准安装路径 / installed-app / system 类别 —— 路径不变量豁免
    InstalledApp,
    /// pm2 托管（God Daemon 后代）—— 用户有意为之
    Pm2Managed,
    /// 启动 < 10s，可能正处于重启/接管的过渡态 —— 降级为存疑
    JustReparented,
}

/// 置信度分层。一键清扫与托盘计数只覆盖 Confirmed + Likely；Possible 永不入清扫。
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Confidence {
    None,
    Possible,
    Likely,
    Confirmed,
}

#[derive(Debug, Clone)]
pub(crate) struct Verdict {
    pub is_suspect: bool,
    pub confidence: Confidence,
    pub reasons: Vec<ReasonCode>,
}

impl Verdict {
    fn clear(reasons: Vec<ReasonCode>) -> Self {
        Verdict {
            is_suspect: false,
            confidence: Confidence::None,
            reasons,
        }
    }
}

/// dev-server 长关键字：足够独特，整行小写子串匹配（双平台共用）——
/// 出现在脚本路径片段里（node_modules/vite/bin/vite.js）同样是真实 dev 证据。
pub(crate) const DEV_SERVER_SUBSTRINGS: &[&str] = &[
    "vite",
    "remix",
    "nuxt",
    "astro",
    "svelte",
    "uvicorn",
    "gunicorn",
    "flask",
    "fastapi",
    "django",
    "hypercorn",
    "ts-node",
    "esbuild",
    "webpack",
    "rspack",
    "turbopack",
    "rollup",
    "parcel",
    "snowpack",
    "tauri",
    "electron",
    "tomcat",
    "jetty",
    "go run",
    "cargo run",
    "cargo-watch",
    "artisan",
    "http.server",
    "live-server",
    "browser-sync",
    "http-server",
    "ngrok",
    "cloudflared",
    "streamlit",
    "gradio",
    "jupyter",
    "next dev",
    "next-server",
    "next start",
    // 运行时名内嵌异词 / 无版本号连字符后缀，token_is 的「数字开头」闸挡不住又确为
    // dev 工具的，在此显式收录（评审发现的回归：旧 "node"/"php" 裸子串曾命中它们）。
    "nodemon",
    "php-fpm",
];

/// dev-server 短关键字：常见词，裸子串会大面积误伤（评审实锤的 Confirmed 误升级：
/// "serve"⊂redis-server/myserver/observer、"node"⊂prometheus-node-exporter、
/// "java"⊂javascript-engine、"bun"⊂ubuntu-report）。只按「token 基名 == 关键字」
/// 匹配，容忍数字/点版本后缀与 .exe：/opt/homebrew/bin/node、python3.12、
/// java.exe 命中；redis-server、javascript-engine、guardrails 不命中。
/// 误伤面收紧的代价（npx serve 等包装丢失 token）由 dev_category 兜底 ——
/// classify 的 dev_like = keyword || category 本就是双保险。
pub(crate) const DEV_SERVER_TOKENS: &[&str] = &[
    "node", "next", "nest", "python", "ruby", "rails", "puma", "unicorn", "deno", "bun", "bunx",
    "tsx", "java", "php", "serve", "air",
];

/// token 基名（去路径、去 .exe）等于关键字，或其后仅是「版本/构建/架构」装饰：
/// 必须以数字（版本号）开头，之后才放行字母数字 / 点 / 连字符 —— node18、python3.12、
/// python3.13t（自由线程构建）、node20.11.0-arm64 命中。
/// 「数字开头」是关键防误伤闸：关键字后直接接异词的 node-exporter、unicorn-tool、
/// javascript（→ "-exporter"/"-tool"/"script-engine" 非数字开头）不命中 ——
/// 真·dev 工具里有此形态的（nodemon、php-fpm）改由 DEV_SERVER_SUBSTRINGS 显式收录。
fn token_is(token: &str, pat: &str) -> bool {
    match strip_exe(basename(token)).strip_prefix(pat) {
        Some("") => true,
        Some(rest) => {
            rest.starts_with(|c: char| c.is_ascii_digit())
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        }
        None => false,
    }
}

pub(crate) fn is_dev_server(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    if DEV_SERVER_SUBSTRINGS.iter().any(|p| lower.contains(p)) {
        return true;
    }
    lower
        .split_whitespace()
        .any(|tok| DEV_SERVER_TOKENS.iter().any(|p| token_is(tok, p)))
}

/// 启动后的宽限期（秒）：刚被收养/刚重启的进程降级为 Possible，防闪报、防误扫。
/// pub(crate)：Windows 采集层净化未知创建时间时复用此阈值（见 windows.rs sanitize_times）——
/// 创建时间读不到 ≠「刚启动」，elapsed 必须落在宽限期之外，否则受保护的孤儿 dev server
/// 会被永久钉在 Possible、永不入清扫/计数（评审发现）。
pub(crate) const GRACE_SECS: u64 = 10;

/// v2 分类器 —— 纯函数：输入信号快照，输出判定。
/// 不变量：标准安装路径 / launchd 托管 ⇒ 永不自动标记（白名单逻辑在 scan() 外层，不在此处）。
pub(crate) fn classify(s: &ProcessSnapshot) -> Verdict {
    // ---- 1. 硬正向：defunct 永远标记 ----
    if s.state.as_deref().is_some_and(|st| st.contains('Z')) {
        return Verdict {
            is_suspect: true,
            confidence: Confidence::Confirmed,
            reasons: vec![ReasonCode::Defunct],
        };
    }

    // ---- 2. 硬豁免（顺序即优先级，先于一切正向信号）----
    if s.launchd_managed {
        return Verdict::clear(vec![ReasonCode::LaunchdManaged]);
    }
    if s.exe_is_standard_install {
        return Verdict::clear(vec![ReasonCode::InstalledApp]);
    }
    if s.brew_service_path {
        return Verdict::clear(vec![ReasonCode::BrewServicePath]);
    }
    if s.pm2_managed {
        return Verdict::clear(vec![ReasonCode::Pm2Managed]);
    }

    // ---- 3. 收集正向信号 ----
    let direct_orphan = s.direct_orphan.is_some();
    let dev_like = s.dev_keyword || s.dev_category;
    let chain_orphan = s.chain_terminates_at_init && (dev_like || s.chain_has_orphan_shell);

    // ---- 4. 无任何孤儿信号 → 不是嫌疑 ----
    if !direct_orphan && !chain_orphan && !s.tty_orphaned {
        return Verdict::clear(vec![]);
    }

    let mut reasons = Vec::new();
    if let Some(code) = s.direct_orphan {
        reasons.push(code);
    }
    if chain_orphan {
        reasons.push(ReasonCode::OrphanedChain);
    }
    if s.tty_orphaned {
        reasons.push(ReasonCode::OrphanedSession);
    }
    if direct_orphan || chain_orphan {
        reasons.push(ReasonCode::NonstandardPath);
    }
    if s.dev_keyword {
        reasons.push(ReasonCode::DevServerKeyword);
    }

    // ---- 5. 宽限期：太年轻 → 存疑，永不入清扫 ----
    if s.elapsed_secs < GRACE_SECS {
        reasons.push(ReasonCode::JustReparented);
        return Verdict {
            is_suspect: true,
            confidence: Confidence::Possible,
            reasons,
        };
    }

    // ---- 6. 置信分层 ----
    // Windows 上「父进程已退出」是常态（Squirrel/Electron 的引导器模式：
    // Update.exe 拉起应用后退出）—— 单独出现时只是弱信号，降到 Possible
    // 永不入清扫；有 dev 特征 / 槽位复用证据 / 链与会话佐证时才升级。
    let weak_parent_exited = s.direct_orphan == Some(ReasonCode::ParentExited)
        && !dev_like
        && !chain_orphan
        && !s.tty_orphaned;
    let confidence =
        if (direct_orphan && (dev_like || s.tty_orphaned)) || (chain_orphan && dev_like) {
            // 孤儿 × dev 特征 / 孤儿 × 会话已死 —— 多信号互证
            Confidence::Confirmed
        } else if weak_parent_exited {
            Confidence::Possible
        } else if direct_orphan || chain_orphan {
            // 孤儿但意图不可证（如 nohup 的非 dev 二进制）；
            // macOS 的 PPID=1 与 Windows 的槽位复用都是强证据，保持 Likely
            Confidence::Likely
        } else {
            // 仅 tty_orphaned：父进程关系尚可解释，单独的会话信号只到存疑
            Confidence::Possible
        };

    Verdict {
        is_suspect: true,
        confidence,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 快捷构造器：默认全 false / 空，按需覆盖。
    fn snap() -> ProcessSnapshot {
        ProcessSnapshot {
            elapsed_secs: 3600,
            ..Default::default()
        }
    }

    struct Case {
        name: &'static str,
        snap: ProcessSnapshot,
        want_suspect: bool,
        want_conf: Confidence,
        /// 判定原因必须包含的码（集合包含校验，与顺序无关）
        want_reasons: &'static [ReasonCode],
    }

    #[test]
    fn classify_fixtures() {
        use Confidence::*;
        use ReasonCode::*;

        let cases = [
            // ============ 不误报（曾经/可能的 FP 全部豁免） ============
            Case {
                name: "1 brew-postgres: launchctl 认领的 PPID=1 服务",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    launchd_managed: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[LaunchdManaged],
            },
            Case {
                name: "2 brew-redis (Intel /usr/local): launchctl 探不到时路径兜底",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    brew_service_path: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[BrewServicePath],
            },
            Case {
                name: "3 LaunchAgent 用户路径助手",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    launchd_managed: true,
                    dev_keyword: true, // 哪怕命中关键字也豁免
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[LaunchdManaged],
            },
            Case {
                name: "6 Terminal 活链里的 vite：无任何孤儿信号",
                snap: ProcessSnapshot {
                    dev_keyword: true,
                    dev_category: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[],
            },
            Case {
                name: "7 Cursor 启动的 node：链上有 installed-app，不触发链孤儿",
                snap: ProcessSnapshot {
                    chain_terminates_at_init: false, // 链在 Cursor 处停下
                    dev_keyword: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[],
            },
            Case {
                name: "8 pm2 托管的 node：有意为之，豁免",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    pm2_managed: true,
                    dev_keyword: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[Pm2Managed],
            },
            Case {
                name: "13 系统组件 cupsd：标准路径不变量",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    exe_is_standard_install: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[InstalledApp],
            },
            Case {
                name: "14 /Applications 裸二进制 PPID=1：installed-app 豁免",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    exe_is_standard_install: true,
                    dev_keyword: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[InstalledApp],
            },
            // ============ 不漏报（曾经的 FN 全部检出） ============
            Case {
                name: "4 直接孤儿 vite（PPID=1 + dev 关键字）",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    dev_keyword: true,
                    dev_category: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[Ppid1Orphan, DevServerKeyword, NonstandardPath],
            },
            Case {
                name: "5 孤儿链 zsh(死)→npm→next：本体 PPID 活着但链根已死 —— 头号漏报修复",
                snap: ProcessSnapshot {
                    chain_terminates_at_init: true,
                    chain_has_orphan_shell: true,
                    dev_keyword: true,
                    dev_category: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[OrphanedChain, DevServerKeyword, NonstandardPath],
            },
            Case {
                name: "9 defunct 僵尸：永远 Confirmed",
                snap: ProcessSnapshot {
                    state: Some("Z".into()),
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[Defunct],
            },
            Case {
                name: "11 终端崩溃留下的 ttys 孤儿（PPID=1 + 会话死 + dev）",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    tty_orphaned: true,
                    dev_keyword: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[Ppid1Orphan, OrphanedSession, DevServerKeyword],
            },
            // ============ 分级灰区 ============
            Case {
                name: "10 刚被收养 4 秒的 vite：宽限期 → Possible，不入清扫",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    dev_keyword: true,
                    elapsed_secs: 4,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Possible,
                want_reasons: &[Ppid1Orphan, JustReparented],
            },
            Case {
                name: "12 nohup 脱离的非 dev 二进制：孤儿但意图不可证 → Likely",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    ..snap()
                },
                want_suspect: true,
                want_conf: Likely,
                want_reasons: &[Ppid1Orphan, NonstandardPath],
            },
            Case {
                name: "15 仅会话信号（tty 死但父链正常）→ Possible",
                snap: ProcessSnapshot {
                    tty_orphaned: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Possible,
                want_reasons: &[OrphanedSession],
            },
            // ============ Windows 语义 ============
            Case {
                name: "16 Win 父进程已退出的 node",
                snap: ProcessSnapshot {
                    direct_orphan: Some(ParentExited),
                    dev_keyword: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[ParentExited, DevServerKeyword],
            },
            Case {
                name: "17 Win PID 槽位复用（父创建时间晚于子）—— 强证据保持 Likely",
                snap: ProcessSnapshot {
                    direct_orphan: Some(PidSlotReused),
                    ..snap()
                },
                want_suspect: true,
                want_conf: Likely,
                want_reasons: &[PidSlotReused, NonstandardPath],
            },
            Case {
                name: "19 Win 裸 ParentExited（无 dev/链/会话佐证）→ Possible，永不入清扫。\
                       Squirrel 应用（Discord/Spotify）即使漏过 installed-app 归类也不会被误杀",
                snap: ProcessSnapshot {
                    direct_orphan: Some(ParentExited),
                    ..snap()
                },
                want_suspect: true,
                want_conf: Possible,
                want_reasons: &[ParentExited, NonstandardPath],
            },
            Case {
                name: "18 Win 正常 cmd→explorer 链的 dev server：explorer 活根挡住链孤儿",
                snap: ProcessSnapshot {
                    chain_terminates_at_init: false, // 活根（explorer）处停下
                    dev_keyword: true,
                    dev_category: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[],
            },
            Case {
                // 真实漏报修复：brew 解释器跑 `-m http.server`，孤儿化后曾被
                // brew_service_path 整体豁免。mod.rs 现按「身份路径」取证 ——
                // 模块调用没有可豁免的脚本路径 ⇒ brew_service_path=false、
                // 类别 dev-script ⇒ 直接孤儿 × dev ⇒ Confirmed 入清扫。
                name: "20 孤儿 python -m http.server（brew 解释器）：身份是模块，必须检出",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    dev_keyword: true,        // "python" 命中 DEV_SERVER_TOKENS
                    dev_category: true,       // identify_app `-m 模块` → dev-script
                    brew_service_path: false, // brew_service_exemption 按模块身份判为不豁免
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[Ppid1Orphan, DevServerKeyword, NonstandardPath],
            },
            Case {
                name: "21 launchd 托管的 python -m 守护（LaunchAgent）：托管豁免优先于一切模块判定",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    launchd_managed: true,
                    dev_keyword: true,
                    dev_category: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[LaunchdManaged],
            },
        ];

        for c in &cases {
            let v = classify(&c.snap);
            assert_eq!(v.is_suspect, c.want_suspect, "[{}] is_suspect", c.name);
            assert_eq!(
                v.confidence, c.want_conf,
                "[{}] confidence (got {:?})",
                c.name, v.confidence
            );
            for r in c.want_reasons {
                assert!(
                    v.reasons.contains(r),
                    "[{}] reasons {:?} 应包含 {:?}",
                    c.name,
                    v.reasons,
                    r
                );
            }
            if c.want_reasons.is_empty() {
                assert!(
                    v.reasons.is_empty(),
                    "[{}] 应无 reasons，得到 {:?}",
                    c.name,
                    v.reasons
                );
            }
        }
    }

    #[test]
    fn dev_keyword_matches_windows_exe_names() {
        assert!(is_dev_server(
            "C:\\Program Files\\nodejs\\node.exe server.js"
        ));
        assert!(is_dev_server("vite"));
        assert!(!is_dev_server("C:\\Windows\\System32\\svchost.exe"));
    }

    /// 回归（评审实锤的误升级面）：短关键字不得因路径/名字里的偶然子串命中。
    /// 曾经的后果：nohup 脱离的 /usr/local/bin/redis-server（手装非 brew）因
    /// "serve" 子串获得 dev_keyword，从 Likely 误升 Confirmed；Windows 上裸
    /// ParentExited 的 myserver.exe 绕过 weak_parent_exited 降级直入清扫。
    #[test]
    fn dev_keyword_short_words_require_exact_command_token() {
        // 误伤面：全部必须不命中
        for fp in [
            "/usr/local/bin/redis-server --port 6379",
            "/Users/x/bin/myserver --listen 8080",
            "/Users/x/bin/observer-daemon",
            "/Users/x/bin/javascript-engine --port 9000",
            "/usr/local/bin/prometheus-node-exporter",
            "/Users/x/.cargo/bin/unicorn-tool",
            "/opt/guardrails/bin/guardrails",
            "/usr/local/bin/ubuntu-report",
            "C:\\Users\\x\\tools\\myserver.exe --port 8080",
            "/Users/x/bin/conserver",
        ] {
            assert!(!is_dev_server(fp), "误伤: {fp}");
        }
        // 真阳性：全部必须保持命中
        for tp in [
            "node",                                        // lsof 短命令名
            "node.exe server.js",                          // Windows 运行时
            "/opt/homebrew/bin/node /Users/x/p/server.js", // exe 全路径 token
            "python3 -m http.server 8000",                 // 版本后缀 + 模块子串
            "Python3.12 app.py",                           // 多级版本后缀
            "java -jar app.jar",
            "serve -s build", // 真·serve CLI
            "air",            // 裸 air（旧 "air " 带尾空格时漏报）
            "bunx vite dev",
            "/Users/x/p/node_modules/.bin/vite --port 5173", // 长词子串路径证据
            "next-server (v14.2.3)",
            // 评审实锤的收紧回归：版本/架构装饰（数字开头）必须仍命中
            "/usr/local/bin/node20.11.0-arm64 server.js",
            "python3.13t app.py", // 自由线程 CPython 构建（t 后缀）
            "php8.2-fpm",         // 版本号 + -fpm
            // 内嵌异词形态由 DEV_SERVER_SUBSTRINGS 兜底
            "nodemon server.js",
            "/usr/sbin/php-fpm",
        ] {
            assert!(is_dev_server(tp), "漏报: {tp}");
        }
    }

    #[test]
    fn reason_codes_serialize_snake_case() {
        // i18n parity 脚本依赖这些 serde 名称稳定
        let json = serde_json::to_string(&ReasonCode::Ppid1Orphan).unwrap();
        assert_eq!(json, "\"ppid1_orphan\"");
        let json = serde_json::to_string(&ReasonCode::DevServerKeyword).unwrap();
        assert_eq!(json, "\"dev_server_keyword\"");
        let json = serde_json::to_string(&Confidence::Confirmed).unwrap();
        assert_eq!(json, "\"confirmed\"");
    }
}
