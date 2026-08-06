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
    /// 一次性自动化浏览器实例（--headless + 调试端口/临时 profile），
    /// 且调试端口上无任何客户端连接 —— 自动化框架已退出、实例无人认领
    AutomationInstance,
    // —— 豁免/降级信号（解释为什么不标记 / 降级）——
    /// 自动化实例的调试端口上有活跃客户端连接 —— 有人正在驱动它，一票否决
    DebuggerAttached,
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

#[derive(Debug)]
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

/// dev-server 长关键字：整行小写、词界锚定的子串匹配（双平台共用）——
/// 出现在脚本路径片段里（node_modules/vite/bin/vite.js）同样是真实 dev 证据。
/// 词界锚定不可省：vite⊂invite、astro⊂gastro/disastrous、remix⊂premix、
/// tauri⊂centauri、electron⊂electronics —— 与 "serve"⊂redis-server 同型的
/// Confirmed 误升级面（评审实锤），只是残留在长词表里；真实 dev 工具在命令行里
/// 两侧总是 / \ . @ - 空白等分隔符，锚定后仍全部命中。
const DEV_SERVER_SUBSTRINGS: &[&str] = &[
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
    // 浏览器自动化工具链（KNOWN-GAPS Gap 1 的同族）：driver 与测试运行器本身占端口
    // （chromedriver 9515 等），孤儿化后就是纯残留。
    "chromedriver",
    "geckodriver",
    "msedgedriver",
    "safaridriver",
    "webdriver",
    "playwright",
    "puppeteer",
    "selenium",
    "cypress",
    "appium",
];

/// dev-server 短关键字：常见词，裸子串会大面积误伤（评审实锤的 Confirmed 误升级：
/// "serve"⊂redis-server/myserver/observer、"node"⊂prometheus-node-exporter、
/// "java"⊂javascript-engine、"bun"⊂ubuntu-report）。只按「token 基名 == 关键字」
/// 匹配，容忍数字/点版本后缀与 .exe：/opt/homebrew/bin/node、python3.12、
/// java.exe 命中；redis-server、javascript-engine、guardrails 不命中。
/// 误伤面收紧的代价（npx serve 等包装丢失 token）由 dev_category 兜底 ——
/// classify 的 dev_like = keyword || category 本就是双保险。
const DEV_SERVER_TOKENS: &[&str] = &[
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

/// 子串命中且两侧邻字符都不是 ASCII 字母数字。needle 全 ASCII；haystack 若含
/// 多字节字符，其任一字节都 >= 0x80、天然通过「非字母数字」边界判定，字节索引安全
/// （needle 命中区间内的 abs+1 必为字符边界）。
fn contains_bounded(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(needle) {
        let abs = start + pos;
        let end = abs + needle.len();
        let before_ok = abs == 0 || !bytes[abs - 1].is_ascii_alphanumeric();
        let after_ok = end == bytes.len() || !bytes[end].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = abs + 1;
    }
    false
}

pub(crate) fn is_dev_server(cmd: &str) -> bool {
    let lower = cmd.to_lowercase();
    if DEV_SERVER_SUBSTRINGS
        .iter()
        .any(|p| contains_bounded(&lower, p))
    {
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
    // 自动化实例的存活性否决（KNOWN-GAPS Gap 1/A2 实测反例）：一个自动化浏览器
    // 存在的意义就是被客户端驱动 —— 调试端口上有 ESTABLISHED 连接 ⇒ 有人正在用它。
    // 这条证据强于任何命令行特征（残留与活跃实例的命令行几乎同构），且**只用于
    // 豁免、不用于升级置信度**：宁可漏报，也不能打断用户正在跑的会话。
    if s.automation_instance && s.debugger_attached {
        return Verdict::clear(vec![ReasonCode::DebuggerAttached]);
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
    // 自动化实例与 dev-script 同权：都是「意图明确的开发期产物」，
    // 孤儿 × 它们 ⇒ Confirmed（浏览器可执行文件装在哪与此无关）。
    let dev_like = s.dev_keyword || s.dev_category || s.automation_instance;
    let chain_orphan = s.chain_terminates_at_init && (dev_like || s.chain_has_orphan_shell);

    // ---- 4. 无任何孤儿信号 → 不是嫌疑 ----
    if !direct_orphan && !chain_orphan && !s.tty_orphaned {
        return Verdict::clear(vec![]);
    }

    let mut reasons = Vec::new();
    if let Some(code) = s.direct_orphan {
        reasons.push(code);
    }
    // OrphanedChain 只在它**独立于**直接孤儿信号时才算一条证据 —— 判据是「链在
    // 终止前有没有真的走过祖先」这个结构事实，不是 direct_orphan 的具体变体。
    //
    // 每个平台的直接孤儿条件（macOS ppid==1；Windows ppid==0 / 父不在表中）都会让
    // build_parent_chain 在**第一次迭代**就终止，一个真实祖先都没走过 —— 此时
    // 「链终止于 init/死根」完全是直接孤儿信号的同义反复，详情面板却把两条并排
    // 列出，读起来像两份独立佐证（真机实测的 macOS 孤儿行即 [Ppid1Orphan,
    // OrphanedChain, ...]）。反之，链走过 zsh→npm 才撞到 launchd（本体 ppid 正常）
    // 时，它是唯一的孤儿证据，必须保留。
    //
    // 评审捕获：按变体写成 `!= Some(Ppid1Orphan)` 会漏掉 Windows 的 ParentExited
    //（它同样蕴含链终止），既留下同样的重复、又把这个 bug 锁进测试；而结构判据
    // 天然覆盖两个平台，且不再让纯分类器依赖 build_parent_chain 的遍历起点。
    // 置信度分层读的是 chain_orphan 变量而非 reasons，故此处不影响任何判定。
    if chain_orphan && s.chain_walked_real_ancestor {
        reasons.push(ReasonCode::OrphanedChain);
    }
    if s.tty_orphaned {
        reasons.push(ReasonCode::OrphanedSession);
    }
    // 仅当 exe 确实不在常规安装位置时才推 —— 走到这里只说明没吃上路径豁免，
    // 而 dev-script / automation-instance 是「身份优先于路径」的例外：它们的
    // exe 常常就装在 /usr/bin、/Applications 里（真机实测：孤儿
    // `/usr/bin/python3 app.py` 的解释器实际解析到 /Applications/Xcode.app/…）。
    // 无条件推入会给这两类最常见的 Confirmed 行贴一条与事实相反的证据。
    //
    // 注意 Homebrew 不在此列：/opt/homebrew/ 本就不属于
    // `is_standard_install_path`（brew 服务另走 brew_service_path 豁免通道），
    // 所以 brew 装的解释器仍会如实拿到这条理由 —— 事实谓词的取证边界完全跟随
    // 平台侧的路径名单，见 ProcessSnapshot::exe_path_is_standard。
    if (direct_orphan || chain_orphan) && !s.exe_path_is_standard {
        reasons.push(ReasonCode::NonstandardPath);
    }
    if s.automation_instance {
        reasons.push(ReasonCode::AutomationInstance);
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
    let confidence = if (direct_orphan || chain_orphan) && (dev_like || s.tty_orphaned) {
        // 孤儿（直接 PPID=1/槽位复用，或链终止于 init）× （dev 特征 / 会话已死）
        // —— 多信号互证。直接孤儿与链孤儿对称享受 tty 佐证（CLAUDE.md：
        // 「orphan × dead-session → Confirmed」），不再厚此薄彼。
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
                    // 链真的走过 npm、zsh 两个祖先才撞到 launchd ⇒ 是独立证据
                    chain_walked_real_ancestor: true,
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
            Case {
                // #2 对称性修复：chain_orphan 此前不享受 tty 佐证，使「链孤儿 ×
                // 会话已死」（非 dev）只判 Likely，与 CLAUDE.md「orphan × dead-session
                // → Confirmed」不符。对称后升 Confirmed。对比 case 15（仅 tty、无孤儿
                // 信号 → 仍 Possible）：这里多了链孤儿这条独立证据。
                name:
                    "22 链孤儿 × 会话已死（非 dev）：dead-session 佐证对直接/链孤儿对称 → Confirmed",
                snap: ProcessSnapshot {
                    chain_terminates_at_init: true,
                    chain_walked_real_ancestor: true, // 走过孤儿 shell 祖先才终止
                    chain_has_orphan_shell: true,     // 非 dev，靠孤儿 shell 祖先成链
                    tty_orphaned: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[OrphanedChain, OrphanedSession, NonstandardPath],
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
                // KNOWN-GAPS Gap 1 主案：headless Chrome 空转 7 小时、子进程满核。
                // exe 在 /Applications，但 mod.rs 已按 automation-instance 把它摘出
                // 路径豁免（exe_is_standard_install=false）—— 到这里就是「孤儿 × dev-like」。
                name: "23 孤儿 headless 自动化实例（无客户端连接）：身份在命令行，必须检出",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    automation_instance: true,
                    debugger_attached: false,
                    // 浏览器本体就住在 /Applications —— 身份例外让它免于路径豁免，
                    // 但路径事实不变（NonstandardPath 因此不在 want_reasons 里，
                    // 专门的不变量见 nonstandard_path_reason_follows_the_actual_exe_path）
                    exe_path_is_standard: true,
                    ..snap()
                },
                want_suspect: true,
                want_conf: Confirmed,
                want_reasons: &[Ppid1Orphan, AutomationInstance],
            },
            Case {
                // KNOWN-GAPS Gap 1/A2 实测反例：判据全中但**有人正在驱动它**。
                // 存活性否决必须先于一切正向信号 —— 误杀会打断用户正在跑的会话。
                name: "24 孤儿自动化实例但调试端口有 ESTABLISHED：存活性一票否决",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    automation_instance: true,
                    debugger_attached: true,
                    dev_keyword: true, // 哪怕有其他正向信号也压不过否决
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[DebuggerAttached],
            },
            Case {
                // 对照：自动化特征本身**不构成**嫌疑 —— 没有孤儿信号就不标记。
                // 活跃会话里的 headless 实例（父进程还在）走这条。
                name: "25 父进程健在的 headless 实例：无孤儿信号 ⇒ 不是嫌疑",
                snap: ProcessSnapshot {
                    automation_instance: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[],
            },
            Case {
                // 托管豁免仍然优先：launchctl 认领的无头浏览器（有人有意常驻）
                name: "26 launchd 托管的 headless 实例：托管豁免优先",
                snap: ProcessSnapshot {
                    direct_orphan: Some(Ppid1Orphan),
                    automation_instance: true,
                    launchd_managed: true,
                    ..snap()
                },
                want_suspect: false,
                want_conf: None,
                want_reasons: &[LaunchdManaged],
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

    /// 证据去重的判据是**链有没有真的走过祖先**，不是 direct_orphan 的变体。
    ///
    /// 两个平台的直接孤儿条件都会让 build_parent_chain 在第一次迭代就终止
    ///（macOS ppid==1；Windows ppid==0 / 父不在表中），此时 OrphanedChain 只是
    /// 把直接孤儿信号换句话重说。评审捕获的回归正在此处：初版按
    /// `!= Some(Ppid1Orphan)` 特判，漏掉 Windows 的 ParentExited 那一半，
    /// 并把「Windows 必须两条并存」写进了断言 —— 本测试锁的是修正后的语义。
    #[test]
    fn orphaned_chain_dedup_keys_on_whether_the_walk_saw_an_ancestor() {
        use ReasonCode::*;

        // 立即终止（没走过任何真实祖先）：两个平台的三种直接孤儿信号都算同义反复
        for direct in [Ppid1Orphan, ParentExited, PidSlotReused] {
            let v = classify(&ProcessSnapshot {
                direct_orphan: Some(direct),
                chain_terminates_at_init: true,
                chain_walked_real_ancestor: false,
                dev_keyword: true,
                ..snap()
            });
            assert!(v.reasons.contains(&direct));
            assert!(
                !v.reasons.contains(&OrphanedChain),
                "{direct:?} 已完整表达链终止，不应再列一条推论：{:?}",
                v.reasons
            );
        }

        // 走过真实祖先后才撞到 init/死根：这是一份独立证据，必须保留 ——
        // 含本体 ppid 正常的链孤儿（zsh→npm→launchd），也含 Windows 上父仍在
        // 进程表、链继续上溯的槽位复用行。
        let v = classify(&ProcessSnapshot {
            chain_terminates_at_init: true,
            chain_walked_real_ancestor: true,
            chain_has_orphan_shell: true,
            dev_keyword: true,
            ..snap()
        });
        assert!(v.reasons.contains(&OrphanedChain), "{:?}", v.reasons);

        let v = classify(&ProcessSnapshot {
            direct_orphan: Some(PidSlotReused),
            chain_terminates_at_init: true,
            chain_walked_real_ancestor: true,
            dev_keyword: true,
            ..snap()
        });
        assert!(v.reasons.contains(&PidSlotReused));
        assert!(v.reasons.contains(&OrphanedChain), "{:?}", v.reasons);
    }

    /// `NonstandardPath` 是说给用户听的事实陈述（i18n reasonTip：「可执行文件不在
    /// 系统 / 应用程序等标准安装位置」），不是「没吃到路径豁免」的同义词。
    ///
    /// 真机复现（2026-08-04）：孤儿 `/usr/bin/python3 devsrv.py` 的 exe 实际解析到
    /// `/Applications/Xcode.app/.../Python` —— 标准得不能再标准，却因 dev-script
    /// 的身份例外走到了正向信号区，被贴上一条与事实相反的证据。automation-instance
    /// 完全同构（KNOWN-GAPS Gap 1 的真机记录里 headless Chrome 也带着这条，
    /// 而它的 exe 就在 /Applications 下）。
    ///
    /// 检出能力不受影响：这条理由从不参与置信度分层，且 REASON_PRIORITY 里排在
    /// 孤儿信号之后 —— 去掉后行内故事与 confidence 都不变，只是详情面板少一条错话。
    #[test]
    fn nonstandard_path_reason_follows_the_actual_exe_path() {
        use Confidence::*;
        use ReasonCode::*;

        // 路径规则的例外一：解释器在标准位置，身份是脚本 —— 仍须检出，但不得
        // 声称路径非标准
        let dev_script_in_standard_path = ProcessSnapshot {
            direct_orphan: Some(Ppid1Orphan),
            dev_keyword: true,
            dev_category: true,
            exe_path_is_standard: true, // /Applications/Xcode.app/.../Python
            ..snap()
        };
        let v = classify(&dev_script_in_standard_path);
        assert_eq!(v.confidence, Confirmed, "检出能力不得因此减弱");
        assert!(v.reasons.contains(&Ppid1Orphan));
        assert!(
            !v.reasons.contains(&NonstandardPath),
            "exe 在标准位置时不得声称非标准路径，实得 {:?}",
            v.reasons
        );

        // 路径规则的例外二：automation-instance 的浏览器本体常住 /Applications
        let automation_in_applications = ProcessSnapshot {
            direct_orphan: Some(Ppid1Orphan),
            automation_instance: true,
            exe_path_is_standard: true,
            ..snap()
        };
        let v = classify(&automation_in_applications);
        assert_eq!(v.confidence, Confirmed);
        assert!(v.reasons.contains(&AutomationInstance));
        assert!(!v.reasons.contains(&NonstandardPath), "{:?}", v.reasons);

        // 反向（exe 真在非标准位置仍须保留这条证据）已由夹具表 case 4/12/17 覆盖，
        // 此处不再重述 —— 本测试只负责表达式表达不了的「必须缺席」。
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

    /// 浏览器自动化工具链的 driver / 测试运行器本身也是 dev 残留（Gap 1 同族）。
    /// 词形独特 + 词界锚定，零误伤面 —— 但仍锁一遍回归。
    #[test]
    fn dev_keyword_covers_browser_automation_toolchain() {
        for tp in [
            "/Users/x/p/node_modules/chromedriver/lib/chromedriver/chromedriver --port=9515",
            "/opt/homebrew/bin/geckodriver --port 4444",
            "C:\\Users\\x\\tools\\msedgedriver.exe",
            "/Users/x/p/node_modules/.bin/playwright test",
            "node /Users/x/p/node_modules/puppeteer/lib/esm/puppeteer/node/ProductLauncher.js",
            "java -jar selenium-server-4.18.jar standalone",
            "/Users/x/Library/Caches/Cypress/13.6.0/Cypress.app/Contents/MacOS/Cypress",
        ] {
            assert!(is_dev_server(tp), "漏报: {tp}");
        }
        // 误伤面：普通浏览器与同形异义的用户程序不得命中
        assert!(!is_dev_server(
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
        ));
        assert!(!is_dev_server("/Users/x/bin/driverless-car-sim"));
    }

    /// 长关键字的词界锚定：普通英文词的内嵌形态不得命中（评审实锤 ——
    /// 与 "serve"⊂redis-server 同型的 Confirmed 误升级面，此前残留在长词表：
    /// 一个 nohup 脱离的 invite-mailer 会经孤儿路径直升 Confirmed 进一键清扫）。
    #[test]
    fn dev_keyword_long_words_require_word_boundary() {
        for fp in [
            "/Users/x/bin/invite-mailer --daemon",   // vite ⊂ invite
            "/opt/tools/gastronomy-planner",         // astro ⊂ gastro
            "/Users/x/bin/disastrous-recovery-tool", // astro ⊂ disastrous
            "/usr/local/bin/premix-audio",           // remix ⊂ premix
            "/Users/x/bin/centauri-sync",            // tauri ⊂ centauri
            "/Users/x/bin/electronics-inventory",    // electron ⊂ electronics
        ] {
            assert!(!is_dev_server(fp), "误伤: {fp}");
        }
        // 真阳性：分隔符（/ \ . @ - 空白）两侧的真实工具形态必须保持命中
        for tp in [
            "node /app/node_modules/vite/bin/vite.js",
            "node /app/node_modules/.pnpm/vite@5.4.0/node_modules/vite/bin/vite.js",
            "npx remix vite:dev",
            "cargo-tauri dev",
            "/app/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron .",
            "node /app/node_modules/astro/astro.js dev",
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
