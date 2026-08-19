use serde::Serialize;

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

impl Confidence {
    /// wire 形态（serde lowercase）的唯一非 serde 出口 —— CLI 表格等人类可读输出
    /// 必须与 JSON 同源。曾用 `format!("{:?}").to_lowercase()` 重构 wire 名：
    /// 四个变体恰好都是单词才侥幸等价，将来任何多词变体（rename_all 与
    /// Debug+lowercase 对多词的结果不同源）会让两种输出静默分叉。
    /// 与 serde 的一致性由 `confidence_as_str_matches_serde_wire_format` 钉住。
    pub fn as_str(&self) -> &'static str {
        match self {
            Confidence::None => "none",
            Confidence::Possible => "possible",
            Confidence::Likely => "likely",
            Confidence::Confirmed => "confirmed",
        }
    }
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

/// 「dev-like」判据的唯一实现 —— 两个消费方必须同源：
///   1. 本文件的置信度分层（孤儿 × dev ⇒ Confirmed）；
///   2. mod.rs 的孤儿预闸 `orphan_gate_dev_like`（无端口行的纳入门槛）。
///
/// 将来新增 dev 信号（新类别、新的命令行证据）只改这里，预闸自动跟上 ——
/// 不存在「分层认、预闸不认」的静默漂移（曾是两份靠约定同步的内联表达式）。
pub(super) fn is_dev_like(
    dev_keyword: bool,
    dev_category: bool,
    automation_instance: bool,
) -> bool {
    // 自动化实例与 dev-script 同权：都是「意图明确的开发期产物」，
    // 孤儿 × 它们 ⇒ Confirmed（浏览器可执行文件装在哪与此无关）。
    dev_keyword || dev_category || automation_instance
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
    //
    // **实际只覆盖一个窄竞态窗口，不是常态路径**（评审发现：此处与 CLAUDE.md 都曾
    // 把它写得像主路径）。稳态僵尸根本喂不到这里：
    //   - 监听者一路：僵尸已释放全部 fd，`lsof -sTCP:LISTEN` 报不到它；
    //   - 无端口孤儿一路：要先过 dev-like 预闸，而 macOS 的 `ps` 对僵尸的
    //     `command` 与 `comm` **都只输出 `<defunct>`**（本机实测 5 个僵尸，无一例外），
    //     进程名整个丢失 ⇒ dev 关键字/类别/自动化三个判据全假 ⇒ 整行被丢弃。
    // 真正会走到这里的是「lsof 与 ps 两次快照之间（~50ms）刚好死掉的监听者」。
    //
    // 这条规则本身是对的，保留：拿到带 Z 的快照就该这么判。但**不要**为了让它
    // 常态生效去放宽孤儿预闸 —— 僵尸已经死了，杀它没有任何效果，该处理的是不回收
    // 子进程的父进程；把它列进一个承诺「杀掉就能拿回端口」的工具里，只会让
    // confirmGone 永远报「仍在」。
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
    let dev_like = is_dev_like(s.dev_keyword, s.dev_category, s.automation_instance);
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
        /// 判定原因的**完整**集合（顺序无关的集合相等断言）。
        /// 曾是「包含」校验 —— 多出一条不该有的 reason 表格察觉不到，而本项目
        /// 修过的两个真 bug（OrphanedChain 同义反复、NonstandardPath 说错话）
        /// 恰恰都是这一类；「必须缺席」只能另写专项测试才表达得了。相等断言让
        /// 表本身就钉住缺席（专项测试保留其回归叙事）。
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
                want_reasons: &[
                    Ppid1Orphan,
                    OrphanedSession,
                    NonstandardPath,
                    DevServerKeyword,
                ],
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
                want_reasons: &[
                    Ppid1Orphan,
                    NonstandardPath,
                    DevServerKeyword,
                    JustReparented,
                ],
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
                want_reasons: &[ParentExited, NonstandardPath, DevServerKeyword],
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
            // 顺序无关的集合相等（ReasonCode 无 Ord，借 Debug 名排序归一）：
            // 多一条、少一条都在此翻红，无需为「必须缺席」另立断言
            let sorted = |rs: &[ReasonCode]| {
                let mut v: Vec<String> = rs.iter().map(|r| format!("{r:?}")).collect();
                v.sort();
                v
            };
            assert_eq!(
                sorted(&v.reasons),
                sorted(c.want_reasons),
                "[{}] reasons 必须与期望集合完全相等（实得 {:?}）",
                c.name,
                v.reasons
            );
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
    fn reason_codes_serialize_snake_case() {
        // i18n parity 脚本依赖这些 serde 名称稳定
        let json = serde_json::to_string(&ReasonCode::Ppid1Orphan).unwrap();
        assert_eq!(json, "\"ppid1_orphan\"");
        let json = serde_json::to_string(&ReasonCode::DevServerKeyword).unwrap();
        assert_eq!(json, "\"dev_server_keyword\"");
        let json = serde_json::to_string(&Confidence::Confirmed).unwrap();
        assert_eq!(json, "\"confirmed\"");
    }

    /// `as_str` 与 serde wire 形态同源 —— 全变体遍历断言，防止 rename_all
    /// 与手写 match 各自演化后 CLI 表格与 JSON 悄悄分叉。
    #[test]
    fn confidence_as_str_matches_serde_wire_format() {
        for c in [
            Confidence::None,
            Confidence::Possible,
            Confidence::Likely,
            Confidence::Confirmed,
        ] {
            let wire = serde_json::to_string(&c).unwrap();
            assert_eq!(wire, format!("\"{}\"", c.as_str()), "{c:?}");
        }
    }
}
