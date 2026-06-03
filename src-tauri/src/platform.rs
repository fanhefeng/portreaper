//! 进程终止的平台实现，带「身份校验」防 PID 复用：
//! scan 时捕获的 start_unix 在 kill 前重新核对，创建时间对不上即拒绝 ——
//! 杀错一个无辜的复用 PID 属于数据损失级事故，宁可让用户重新扫描。
//!
//! 错误信息以 `ERR_*:` 前缀开头的是本应用语义错误（前端可 i18n 映射），
//! 其余为操作系统原文。

/// 创建时间容差（秒）：macOS 两侧都由 `now - etime` 推导，存在 ±1~2s 抖动；
/// 被复用的 PID 创建时间必然晚于扫描时刻，远超此容差。
const START_TOLERANCE_SECS: u64 = 5;

#[cfg(target_os = "macos")]
pub fn kill(pid: u32, force: bool, expected_start: Option<u64>) -> Result<(), String> {
    use std::process::Command;

    // fail-closed：没有身份令牌就拒绝（scan() 保证每行都带 start_unix，
    // 走到这里说明前端数据异常 —— 宁可让用户重扫，绝不盲杀）
    let expected = expected_start
        .ok_or_else(|| "ERR_IDENTITY_UNKNOWN: missing identity token, rescan first".to_string())?;
    let current = current_start_unix(pid)
        .ok_or_else(|| "ERR_PROCESS_GONE: process no longer exists".to_string())?;
    if current.abs_diff(expected) > START_TOLERANCE_SECS {
        return Err(
            "ERR_PID_REUSED: process identity changed (PID was reused), rescan and retry"
                .to_string(),
        );
    }
    // 已知残余竞态：ps 校验与 kill 是两个独立子进程，二者之间存在亚毫秒级
    // 窗口（macOS 无法像 Windows 那样用同一句柄钉住身份）。PID 在该窗口内
    // 被复用且新进程创建时间恰落在 ±5s 容差内的概率可忽略 —— 接受并记录。

    let signal = if force { "-9" } else { "-15" };
    let output = Command::new("kill")
        .args([signal, &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    Ok(())
}

/// 单 PID 的当前创建时间（epoch 秒）：now - etime。进程不存在返回 None。
#[cfg(target_os = "macos")]
fn current_start_unix(pid: u32) -> Option<u64> {
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    let output = Command::new("ps")
        .args(["-o", "etime=", "-p", &pid.to_string()])
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let etime = text.trim();
    if etime.is_empty() {
        return None;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Some(now.saturating_sub(crate::scanner::parse_etime_secs(etime)))
}

#[cfg(windows)]
pub fn kill(pid: u32, _force: bool, expected_start: Option<u64>) -> Result<(), String> {
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
    let expected = expected_start
        .ok_or_else(|| "ERR_IDENTITY_UNKNOWN: missing identity token, rescan first".to_string())?;

    unsafe {
        let handle = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
        .map_err(|e| {
            // ERROR_INVALID_PARAMETER (87) = PID 已不存在 → 映射为语义错误供前端 i18n；
            // 其余（如 5 access denied = 被策略/EDR 拒绝）透传 Win32 原文
            if e.code() == windows::Win32::Foundation::ERROR_INVALID_PARAMETER.to_hresult() {
                "ERR_PROCESS_GONE: process no longer exists".to_string()
            } else {
                format!("OpenProcess({pid}) failed: {e}")
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
            .map_err(|e| format!("GetProcessTimes({pid}) failed: {e}"))?;
        let current = filetime_to_unix(creation);
        if current.abs_diff(expected) > START_TOLERANCE_SECS {
            return Err(
                "ERR_PID_REUSED: process identity changed (PID was reused), rescan and retry"
                    .to_string(),
            );
        }

        TerminateProcess(handle, 1).map_err(|e| format!("TerminateProcess({pid}) failed: {e}"))?;
    }
    Ok(())
}
