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

    if let Some(expected) = expected_start {
        let current = current_start_unix(pid)
            .ok_or_else(|| "ERR_PROCESS_GONE: process no longer exists".to_string())?;
        if current.abs_diff(expected) > START_TOLERANCE_SECS {
            return Err(
                "ERR_PID_REUSED: process identity changed (PID was reused), rescan and retry"
                    .to_string(),
            );
        }
    }

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

    unsafe {
        let handle = OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
        // 精确透传 Win32 错误码：让用户能区分「进程已不在」(87/  invalid param)
        // 与「被策略/EDR 拒绝」(5/access denied)
        .map_err(|e| format!("OpenProcess({pid}) failed: {e}"))?;
        let _guard = Guard(handle);

        if let Some(expected) = expected_start {
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
        }

        TerminateProcess(handle, 1).map_err(|e| format!("TerminateProcess({pid}) failed: {e}"))?;
    }
    Ok(())
}
