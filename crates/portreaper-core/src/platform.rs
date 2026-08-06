//! 进程终止的平台实现，带「身份校验」防 PID 复用：
//! scan 时捕获的 start_unix 在 kill 前重新核对，创建时间对不上即拒绝 ——
//! 杀错一个无辜的复用 PID 属于数据损失级事故，宁可让用户重新扫描。
//!
//! 失败以 [`KillError`] 返回：语义分支是**枚举**而非字符串前缀。多一个前端就
//! 多一份 `startsWith("ERR_…")` 解析、且没有任何编译期保护 —— 加一个变体时
//! 漏改某个前端，那里只会安静地把语义错误当成 OS 原文透传。
//!
//! 旧的 `ERR_*:` 字符串形态由 [`KillError::to_legacy_string`] 保留，供尚未
//! 迁移到结构化错误的 IPC 边界使用（见 `src-tauri/src/commands.rs`）。

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

    /// 旧 IPC 契约的字符串形态。前端以 `includes("ERR_…")` 匹配，故这些 token
    /// **必须逐字保留**（`src/model.ts` 的 killErrorText 依赖它们）。
    pub fn to_legacy_string(&self) -> String {
        match self {
            Self::IdentityUnknown => {
                "ERR_IDENTITY_UNKNOWN: missing identity token, rescan first".to_string()
            }
            Self::ProcessGone => "ERR_PROCESS_GONE: process no longer exists".to_string(),
            Self::PidReused => {
                "ERR_PID_REUSED: process identity changed (PID was reused), rescan and retry"
                    .to_string()
            }
            Self::AccessDenied => {
                "ERR_ACCESS_DENIED: not permitted to terminate (protected process?)".to_string()
            }
            Self::Os { message } => message.clone(),
        }
    }
}

impl fmt::Display for KillError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_legacy_string())
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
    // 解析失败返回 Ok(None) → 上层 kill 走 ERR_PROCESS_GONE 让用户重扫,绝不静默把
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
mod legacy_contract_tests {
    use super::KillError;

    /// `src/model.ts` 的 killErrorText 用 `err.includes("ERR_…")` 分派本地化文案。
    /// 这些 token 是**跨进程契约**：改一个字母，前端不会报错，只会安静地把语义
    /// 错误当成 OS 原文原样吐给用户（英文、且带实现细节）。故逐条钉死。
    #[test]
    fn legacy_tokens_match_frontend_matchers() {
        let cases = [
            (KillError::IdentityUnknown, "ERR_IDENTITY_UNKNOWN"),
            (KillError::ProcessGone, "ERR_PROCESS_GONE"),
            (KillError::PidReused, "ERR_PID_REUSED"),
            (KillError::AccessDenied, "ERR_ACCESS_DENIED"),
        ];
        for (err, token) in cases {
            let s = err.to_legacy_string();
            assert!(
                s.contains(token),
                "{err:?} 的兼容字符串必须含 {token}，实际: {s}"
            );
        }
    }

    /// OS 原文不得被套上任何 `ERR_` 前缀 —— 那会让前端把一条无语义的系统错误
    /// 误判成某个语义分支（`includes` 是子串匹配，不看位置）。
    #[test]
    fn os_errors_pass_through_verbatim() {
        let err = KillError::os("Operation not permitted");
        assert_eq!(err.to_legacy_string(), "Operation not permitted");
        assert!(!err.to_legacy_string().contains("ERR_"));
    }

    /// 结构化形态的 serde 键名同样是契约（未来的 CLI/Raycast 直接吃 JSON）。
    #[test]
    fn serializes_as_tagged_snake_case() {
        let json = serde_json::to_string(&KillError::PidReused).unwrap();
        assert_eq!(json, r#"{"code":"pid_reused"}"#);
        let json = serde_json::to_string(&KillError::os("boom")).unwrap();
        assert_eq!(json, r#"{"code":"os","message":"boom"}"#);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod live_tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    /// 真机验证 kill 身份校验（默认忽略：cargo test kill_identity -- --ignored）：
    /// 错误令牌必须拒绝（ERR_PID_REUSED）、缺令牌必须拒绝（ERR_IDENTITY_UNKNOWN）、
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
