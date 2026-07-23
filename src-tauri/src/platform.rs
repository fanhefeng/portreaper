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
    // 探针工具本身失败（ps 起不来）≠ 进程消失：前者以 OS 原文上抛，
    // 不映射成 ERR_PROCESS_GONE 误导用户「进程已不在」（评审发现）。
    let current = current_start_unix(pid)
        .map_err(|e| format!("verify process identity: {e}"))?
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
    // 固定绝对路径（纵深防御）：不经 $PATH 解析，避免被劫持的 `kill` 二进制
    // 在用户每次点「终止」时执行任意代码（评审发现）。映射与 scanner 同源，
    // 避免两处各写绝对路径而漂移（crate::scanner::system_bin）。
    let output = Command::new(crate::scanner::system_bin("kill"))
        .args([signal, &pid.to_string()])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        // /bin/kill 失败但 stderr 为空（个别 EPERM 场景）时给出带退出码的兜底
        // 文案 —— 否则前端错误横幅展示空字符串（评审发现）。
        if stderr.is_empty() {
            return Err(format!("kill {pid} failed with {}", output.status));
        }
        return Err(stderr.to_string());
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
            use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER};
            // ERROR_INVALID_PARAMETER (87) = PID 已不存在 → 进程已消失。
            // ERROR_ACCESS_DENIED (5) = 进程仍在、但被策略/EDR/受保护进程拒绝 —— 绝不能
            // 谎称「已消失/身份已变」误导用户，给出语义准确的本地化「无权终止」。
            // 两者都映射为 ERR_ 语义码供前端 i18n；其余透传 Win32 原文。
            if e.code() == ERROR_INVALID_PARAMETER.to_hresult() {
                "ERR_PROCESS_GONE: process no longer exists".to_string()
            } else if e.code() == ERROR_ACCESS_DENIED.to_hresult() {
                "ERR_ACCESS_DENIED: not permitted to terminate (protected process?)".to_string()
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
        assert!(err.contains("ERR_IDENTITY_UNKNOWN"), "got: {err}");

        // 2. 错误令牌（伪造一个 1 小时前的创建时间）→ 拒绝
        let err = super::kill(pid, false, Some(now - 3600)).unwrap_err();
        assert!(err.contains("ERR_PID_REUSED"), "got: {err}");

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
