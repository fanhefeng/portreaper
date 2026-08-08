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
    let current = current_start_unix(pid)
        .map_err(|e| KillError::os(format!("verify process identity: {e}")))?
        .ok_or(KillError::ProcessGone)?;
    if current.abs_diff(expected) > START_TOLERANCE_SECS {
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
    Ok(())
}

/// 单 PID 的当前创建时间（epoch 秒）：now - etime。
/// `Ok(None)` = 进程不存在（或 etime 不可解析，fail-closed 同样按消失处理）；
/// `Err` = 探针工具自身失败（ps 无法启动），语义与「进程消失」严格区分。
#[cfg(target_os = "macos")]
fn current_start_unix(pid: u32) -> Result<Option<u64>, String> {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let output = Command::new(crate::scanner::system_bin("ps"))
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .map_err(|e| format!("spawn ps: {e}"))?;
    let text = String::from_utf8_lossy(&output.stdout);
    let etime = text.trim();
    if etime.is_empty() {
        return Ok(None);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 解析失败返回 Ok(None) → 上层 kill 走 process_gone 让用户重扫,绝不静默把
    // 进程当成「刚启动」(current ≈ now 会绕过 ±5s PID 复用容差)。
    Ok(crate::scanner::parse_etime_checked(etime).map(|elapsed| now.saturating_sub(elapsed)))
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
}
