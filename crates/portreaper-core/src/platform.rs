//! 进程终止的平台实现，带「身份校验」防 PID 复用：
//! scan 时捕获的 start_unix 在 kill 前重新核对，创建时间对不上即拒绝 ——
//! 杀错一个无辜的复用 PID 属于数据损失级事故，宁可让用户重新扫描。
//!
//! 失败以 [`KillError`] 返回：语义分支是**枚举**而非字符串前缀。多一个前端就
//! 多一份 `startsWith("ERR_…")` 解析、且没有任何编译期保护 —— 加一个变体时
//! 漏改某个前端，那里只会安静地把语义错误当成 OS 原文透传。
//!
//! **所有 IPC 边界都吃 serde 形态 `{code, message?}`**：Tauri 命令直接返回
//! `Result<(), KillError>`（`src-tauri/src/commands.rs`），CLI 把同一个值
//! `serde_json` 写到 stderr（`portreaper-cli/src/main.rs`）。v0.9.0 之前桌面侧
//! 曾走一层 `ERR_*:` 前缀字符串的降级兼容层，已随本次统一删除。

use std::fmt;

use serde::Serialize;

/// 当前构建目标的平台名 —— 三个前端共用的**唯一**推导。
///
/// 曾在 `portreaper-cli` 与 `src-tauri/commands.rs` 各写一份 cfg 判断，且第三分支
/// 还不一致（CLI 给 `unknown`，GUI 给 `windows`）——两者最终喂给同一族消费者
/// （`ScanReport.platform` 与前端的 `Os`），这种分叉迟早会变成「同一台机器两个前端
/// 说法不同」（评审发现）。
///
/// 本函数陈述事实，`unknown` 是诚实的第三档；要不要把它收窄成某个具体平台，
/// 是各前端自己的展示策略（GUI 侧的取舍写在 `get_platform` 上）。
pub fn platform_name() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "unknown"
    }
}

/// 创建时间容差（秒）：macOS 两侧都由 `now - etime` 推导，存在 ±1~2s 抖动；
/// 被复用的 PID 创建时间必然晚于扫描时刻，远超此容差。
const START_TOLERANCE_SECS: u64 = 5;

/// 终止进程失败的原因。
///
/// 前四个变体是**应用语义**，各前端据此分叉 UI 与本地化文案；`Os` 是无语义的
/// 系统原文兜底，只能原样展示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum KillError {
    /// 前端没带扫描时的身份令牌 —— fail-closed，绝不盲杀
    IdentityUnknown,
    /// 目标进程已不存在
    ProcessGone,
    /// 创建时间对不上：PID 已被复用，杀下去就是误伤
    PidReused,
    /// 进程仍在，但被策略 / EDR / 受保护进程拒绝
    AccessDenied,
    /// 操作系统原文，无语义
    Os { message: String },
}

impl KillError {
    fn os(message: impl Into<String>) -> Self {
        Self::Os {
            message: message.into(),
        }
    }
}

/// 面向**日志与 `std::error::Error`** 的英文原文，不是 IPC 契约 —— 前端的
/// 用户可见文案一律由 `code` 分派本地化，绝不解析这里的句子。故意不带任何
/// `ERR_` 前缀：前缀曾是前端 `includes()` 的匹配依据，留着只会诱使人再写一次
/// 字符串匹配。
impl fmt::Display for KillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IdentityUnknown => f.write_str("missing identity token, rescan first"),
            Self::ProcessGone => f.write_str("process no longer exists"),
            Self::PidReused => {
                f.write_str("process identity changed (PID was reused), rescan and retry")
            }
            Self::AccessDenied => f.write_str("not permitted to terminate (protected process?)"),
            Self::Os { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for KillError {}

#[cfg(target_os = "macos")]
pub fn kill(pid: u32, force: bool, expected_start: Option<u64>) -> Result<(), KillError> {
    // fail-closed：没有身份令牌就拒绝（scan() 保证每行都带 start_unix，
    // 走到这里说明前端数据异常 —— 宁可让用户重扫，绝不盲杀）
    let expected = expected_start.ok_or(KillError::IdentityUnknown)?;
    // 探针工具本身失败（ps 起不来）≠ 进程消失：前者以 OS 原文上抛，
    // 不映射成 ProcessGone 误导用户「进程已不在」（评审发现）。
    let probe = probe_identity(pid)
        .map_err(|e| KillError::os(format!("verify process identity: {e}")))?
        .ok_or(KillError::ProcessGone)?;
    if probe.start_unix.abs_diff(expected) > START_TOLERANCE_SECS {
        return Err(KillError::PidReused);
    }
    // 已知残余竞态：ps 校验与 kill(2) 之间存在亚毫秒级窗口（macOS 无法像
    // Windows 那样用同一句柄钉住身份）。PID 在该窗口内被复用且新进程创建
    // 时间恰落在 ±5s 容差内的概率可忽略 —— 接受并记录。

    // 直接 kill(2) syscall 而非 /bin/kill 子进程：errno 可精确映射语义变体
    // （EPERM → AccessDenied、ESRCH → ProcessGone），与 Windows 分支的错误
    // 协议对称 —— 此前 EPERM 以英文 OS 原文透传、不进 i18n（评审发现）；
    // 同时消灭一次 fork/exec 和「kill 二进制被劫持」的攻击面（原 system_bin
    // 绝对路径加固所防的目标）。pid 来自扫描产出且恒为正，不存在落入
    // kill(0)/kill(-1) 广播语义的输入。
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    if unsafe { libc::kill(pid as libc::pid_t, signal) } != 0 {
        let err = std::io::Error::last_os_error();
        return Err(match err.raw_os_error() {
            Some(libc::EPERM) => KillError::AccessDenied,
            Some(libc::ESRCH) => KillError::ProcessGone,
            _ => KillError::os(err.to_string()),
        });
    }
    if !force && probe.stopped {
        resume_after_term(pid);
    }
    Ok(())
}

/// 被挂起（ps state 含 `T`）的进程收不到**已捕获**的 SIGTERM —— 信号一直挂在
/// pending 集里，`kill(2)` 照样返回 0，前端报「已终止」，进程却纹丝不动。
/// 这是「从 Terminal 里按过 Ctrl-Z / 被 SIGTTIN·SIGTTOU 停住的开发服务器杀不掉」
/// 的根因，实测复现：装了 SIGTERM handler 的进程 `kill -STOP` 后再 `kill -TERM`
/// 永远停在 `T`，补一发 SIGCONT 才在同一毫秒里执行 handler 退出。
/// （对照组：未捕获 SIGTERM 的进程默认动作是终止，内核直接杀，无需唤醒；
/// SIGKILL 同理不可捕获，故 force 分支不走这里。）
///
/// 三条自我约束：
/// - **顺序必须 TERM → CONT**：先唤醒会给目标一个「醒着且没收到终止请求」的窗口；
/// - **只在确认 stopped 时发**：SIGCONT 对正常进程虽属无害，但 shell / tmux / TUI
///   常自己装 SIGCONT handler 去重绘，无差别广播等于给每次温和终止附赠一次副作用；
/// - **返回值一律忽略**：TERM 已经成功送达，唤醒失败（进程恰好已退出 → ESRCH）
///   绝不能把一次成功的终止翻成失败。
#[cfg(target_os = "macos")]
fn resume_after_term(pid: u32) {
    unsafe {
        libc::kill(pid as libc::pid_t, libc::SIGCONT);
    }
}

/// kill 前的一次性身份探针结果。
#[cfg(target_os = "macos")]
struct Probe {
    /// 进程创建时间（epoch 秒）：now - etime
    start_unix: u64,
    /// ps state 含 `T` —— 进程被挂起，见 [`resume_after_term`]
    stopped: bool,
}

/// 单 PID 的创建时间 + 运行状态，一次 `ps` 拿全（**不额外多起一个进程**：
/// 状态列是顺带的，`-o etime=,state=` 与原来的 `-o etime=` 同一次 fork/exec）。
/// `Ok(None)` = 进程不存在（或 etime 不可解析，fail-closed 同样按消失处理）；
/// `Err` = 探针工具自身失败（ps 无法启动），语义与「进程消失」严格区分。
#[cfg(target_os = "macos")]
fn probe_identity(pid: u32) -> Result<Option<Probe>, String> {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let output = Command::new(crate::scanner::system_bin("ps"))
        .args(["-o", "etime=,state=", "-p", &pid.to_string()])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .map_err(|e| format!("spawn ps: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(parse_probe_line(&text, now))
}

/// `ps -o etime=,state= -p <pid>` 的一行（形如 `"  00:12 TN  "`）→ [`Probe`]。
/// 抽成纯函数是为了能单测 —— 真正跑 ps 的那半截在 CI 上无法造出 stopped 进程。
///
/// 解析失败返回 None → 上层 kill 走 process_gone 让用户重扫，绝不静默把进程
/// 当成「刚启动」（current ≈ now 会绕过 ±5s PID 复用容差）。
/// state 列缺失/异常时 `stopped` 保守取 false：宁可不唤醒，也不凭猜测发信号。
#[cfg(target_os = "macos")]
fn parse_probe_line(line: &str, now: u64) -> Option<Probe> {
    let mut cols = line.split_whitespace();
    let etime = cols.next()?;
    let state = cols.next().unwrap_or("");
    let elapsed = crate::scanner::parse_etime_checked(etime)?;
    Some(Probe {
        start_unix: now.saturating_sub(elapsed),
        stopped: state.contains('T'),
    })
}

#[cfg(windows)]
pub fn kill(pid: u32, _force: bool, expected_start: Option<u64>) -> Result<(), KillError> {
    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };

    /// RAII 句柄守卫，任何提前 return 都会 CloseHandle
    struct Guard(HANDLE);
    impl Drop for Guard {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn filetime_to_unix(ft: FILETIME) -> u64 {
        // FILETIME：自 1601-01-01 起的 100ns；与 Unix epoch 差 11644473600 秒
        let ticks = ((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64;
        (ticks / 10_000_000).saturating_sub(11_644_473_600)
    }

    // fail-closed：没有身份令牌就拒绝（与 macOS 分支一致）
    let expected = expected_start.ok_or(KillError::IdentityUnknown)?;

    unsafe {
        let handle = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
        .map_err(|e| {
            use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
            // ERROR_INVALID_PARAMETER (87) = PID 已不存在 → 进程已消失。
            // ERROR_ACCESS_DENIED (5) = 进程仍在、但被策略/EDR/受保护进程拒绝 —— 绝不能
            // 谎称「已消失/身份已变」误导用户，给出语义准确的本地化「无权终止」。
            // 两者都映射为语义变体供前端 i18n；其余透传 Win32 原文。
            if e.code() == ERROR_INVALID_PARAMETER.to_hresult() {
                KillError::ProcessGone
            } else if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                KillError::AccessDenied
            } else {
                KillError::os(format!("OpenProcess({pid}) failed: {e}"))
            }
        })?;
        let _guard = Guard(handle);

        // 同一句柄上先校验创建时间、再 TerminateProcess —— 句柄钉住进程身份，
        // 即使 PID 在此期间被复用，句柄仍指向原进程，无 TOCTOU 窗口
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user)
            .map_err(|e| KillError::os(format!("GetProcessTimes({pid}) failed: {e}")))?;
        let current = filetime_to_unix(creation);
        if current.abs_diff(expected) > START_TOLERANCE_SECS {
            return Err(KillError::PidReused);
        }

        TerminateProcess(handle, 1)
            .map_err(|e| KillError::os(format!("TerminateProcess({pid}) failed: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod wire_contract_tests {
    use super::KillError;

    /// `code` 是**唯一的跨进程契约**：桌面前端（`src/model.ts` localizeKillError）
    /// 与 Raycast（`integrations/raycast/src/cli.ts` killErrorMessage）都按它分派
    /// 本地化文案。改一个字母，两侧都不会报错，只会安静地退化成「透传 OS 原文」
    /// —— 用户看到的是英文实现细节。故四个语义码逐条钉死。
    ///
    /// 新增变体时这张表**必须同步扩**，且两个前端的 switch 都要加分支。
    #[test]
    fn semantic_codes_match_frontend_matchers() {
        let cases = [
            (KillError::IdentityUnknown, r#"{"code":"identity_unknown"}"#),
            (KillError::ProcessGone, r#"{"code":"process_gone"}"#),
            (KillError::PidReused, r#"{"code":"pid_reused"}"#),
            (KillError::AccessDenied, r#"{"code":"access_denied"}"#),
        ];
        for (err, expected) in cases {
            let json = serde_json::to_string(&err).unwrap();
            assert_eq!(json, expected, "{err:?} 的 wire 形态变了，前端会静默退化");
        }
    }

    /// `Os` 变体多带一个 `message`：它无语义，前端只能原样展示，故原文必须
    /// 完整过河（截断/改写会让用户拿不到可搜索的系统错误）。
    #[test]
    fn os_variant_carries_verbatim_message() {
        let json = serde_json::to_string(&KillError::os("Operation not permitted")).unwrap();
        assert_eq!(json, r#"{"code":"os","message":"Operation not permitted"}"#);
    }

    /// Display 面向日志，**不得**再带 `ERR_` 前缀 —— 那是已删除的旧字符串契约的
    /// 残迹，留着会诱使人在前端重新写一次 `includes()` 匹配。
    #[test]
    fn display_is_log_text_without_legacy_prefix() {
        for err in [
            KillError::IdentityUnknown,
            KillError::ProcessGone,
            KillError::PidReused,
            KillError::AccessDenied,
            KillError::os("boom"),
        ] {
            assert!(
                !err.to_string().contains("ERR_"),
                "{err:?} 的 Display 仍带旧前缀"
            );
        }
        assert_eq!(KillError::os("boom").to_string(), "boom");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod probe_tests {
    use super::parse_probe_line;

    /// `ps -o etime=,state=` 的真实排版：前导空格 + 列间空格 + 行尾空格。
    /// 逐个 etime 形态钉死 —— 这条解析一旦错，kill 会把「进程还在」读成
    /// 「进程已消失」，用户看到的是一句莫名其妙的 process_gone。
    #[test]
    fn parses_real_ps_layout() {
        let now = 1_000_000;
        for (line, want_elapsed) in [
            ("  00:12 SN  \n", 12),
            (" 01:02:03 Ss+ \n", 3723),
            ("2-03:04:05 R  \n", 183_845),
            ("      05 S\n", 5),
        ] {
            let p = parse_probe_line(line, now).expect("应当解析成功");
            assert_eq!(p.start_unix, now - want_elapsed, "{line:?}");
        }
    }

    /// stopped 的判定只看 state 列里有没有 `T`，且**绝不能**误命中 etime 列
    /// 或其它状态字母（`R`/`S`/`I`/`U`/`Z` 与附加标志 `s`/`+`/`N`/`L`/`W`）。
    #[test]
    fn stopped_is_detected_only_from_the_state_column() {
        let now = 1_000_000;
        for (line, want_stopped) in [
            ("  00:12 TN  \n", true),  // Ctrl-Z 挂起
            ("  00:12 T   \n", true),  // 纯 stopped
            ("  00:12 SN  \n", false), // 正常睡眠
            ("  00:12 Ss+ \n", false), // 会话首进程 + 前台
            ("  00:12 R   \n", false),
            ("  00:12 Z   \n", false), // defunct 不是 stopped
        ] {
            let p = parse_probe_line(line, now).expect("应当解析成功");
            assert_eq!(p.stopped, want_stopped, "{line:?}");
        }
    }

    /// 进程不存在时 ps 输出空 → None（上层映射为 process_gone）；
    /// etime 不可解析同样 None（fail-closed，绝不当成「刚启动」绕过复用容差）。
    #[test]
    fn unparseable_input_is_none() {
        assert!(parse_probe_line("", 1).is_none());
        assert!(parse_probe_line("   \n", 1).is_none());
        assert!(parse_probe_line("garbage TN\n", 1).is_none());
    }

    /// state 列缺失（罕见排版 / 未来的 ps 变体）时必须保守取 false ——
    /// 宁可不唤醒，也不凭猜测给一个正常进程发 SIGCONT。
    #[test]
    fn missing_state_column_is_not_stopped() {
        let p = parse_probe_line("  00:12\n", 100).expect("etime 仍应解析");
        assert!(!p.stopped);
        assert_eq!(p.start_unix, 88);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod live_tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 真机验证 kill 身份校验（默认忽略：cargo test kill_identity -- --ignored）：
    /// 错误令牌必须拒绝（pid_reused）、缺令牌必须拒绝（identity_unknown）、
    /// 正确令牌应放行并真正终止目标。
    #[test]
    #[ignore]
    fn kill_identity_verification() {
        use std::process::Command;
        let mut child = Command::new("sleep").arg("300").spawn().unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_secs(1));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // 1. 缺令牌 → fail-closed
        let err = super::kill(pid, false, None).unwrap_err();
        assert_eq!(err, super::KillError::IdentityUnknown);

        // 2. 错误令牌（伪造一个 1 小时前的创建时间）→ 拒绝
        let err = super::kill(pid, false, Some(now - 3600)).unwrap_err();
        assert_eq!(err, super::KillError::PidReused);

        // 3. 正确令牌（刚创建，约 now-1）→ 放行并终止。
        //    被杀的直接子进程在父 wait() 回收前是 defunct，先 reap 再断言。
        super::kill(pid, true, Some(now - 1)).expect("correct token should kill");
        let status = child.wait().expect("reap killed child");
        assert!(!status.success(), "child should have been killed");
        let alive = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&alive.stdout).trim().is_empty(),
            "process should be gone after reaping"
        );
    }

    /// 真机验证「挂起的进程也能被温和终止」（cargo test kill_stopped -- --ignored）。
    ///
    /// 这是本项目最容易复发的一类 bug：`libc::kill` 返回 0 就当成功，而**被挂起
    /// 且捕获了 SIGTERM** 的进程只是把信号挂进 pending 集，永远不死。用户从
    /// Terminal 里按过 Ctrl-Z 的 dev server 正是这个形态 —— 界面报「已终止」，
    /// 进程纹丝不动。回归靠这条测试守住：删掉 `resume_after_term` 它必然翻红。
    ///
    /// 目标必须**捕获** SIGTERM —— 默认处置是终止的进程（如 `sleep`）内核会直接
    /// 杀掉，即便处于 stopped 态也不需要唤醒，用它当样本测不出任何东西。
    #[test]
    #[ignore]
    fn kill_stopped_process_that_catches_sigterm() {
        use std::process::Command;

        // perl 是 macOS 自带的；`$SIG{TERM}` 让它成为「捕获 SIGTERM」的样本
        let mut child = Command::new("/usr/bin/perl")
            .args(["-e", "$SIG{TERM} = sub { exit 0 }; sleep 300"])
            .spawn()
            .unwrap();
        let pid = child.id();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        // 制造「Terminal 里按了 Ctrl-Z」的现场
        assert_eq!(unsafe { libc::kill(pid as libc::pid_t, libc::SIGSTOP) }, 0);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let state = Command::new("/bin/ps")
            .args(["-o", "state=", "-p", &pid.to_string()])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&state.stdout).contains('T'),
            "样本没进 stopped 态，这条测试就没有在测它该测的东西"
        );

        // 温和终止（非 force）：必须真的死掉，而不只是「信号送到了」
        super::kill(pid, false, Some(now)).expect("stopped 进程的温和终止不应报错");

        // 轮询用 try_wait 而非 ps + wait：目标是本进程的直接子进程，死后在被
        // reap 前是 defunct（ps 仍列出它），而阻塞式 wait 在**测试失败时会永远
        // 挂住** —— 挂起的进程根本不会退出。try_wait 顺带完成回收，两个问题一起
        // 解决。（写这条测试时确实先踩了那个死等，回归验证跑成了一次超时。）
        let mut exited = false;
        for _ in 0..40 {
            if child.try_wait().unwrap().is_some() {
                exited = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        if !exited {
            // 断言之前先收尸：测试失败也不该在机器上留下一个挂起的孤儿
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            let _ = child.wait();
        }
        assert!(
            exited,
            "SIGTERM 送达了但进程还在 —— 挂起态的唤醒（SIGCONT）没生效"
        );
    }
}
