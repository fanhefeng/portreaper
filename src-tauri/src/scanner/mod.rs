//! 扫描编排：平台 provider 采集 → 信号快照 → 纯分类器 → 父链 → 排序。
//! commands.rs 只依赖本文件的 `scan()` 与 `ProcessEntry`。

mod classify;
mod identify;
mod model;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform_impl;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform_impl;

use std::collections::HashMap;

pub use model::ProcessEntry;

/// 供 platform::kill 的身份校验复用（macOS：kill 前用 `ps -o etime=` 重读创建时间）。
#[cfg(target_os = "macos")]
pub(crate) use macos::parse_etime as parse_etime_secs;

use classify::{classify, is_dev_server};
use identify::basename;
use model::{ParentRef, ProcMeta, ProcessSnapshot};

/// 父链回溯的同时收集的孤儿信号。
#[derive(Default)]
struct ChainFlags {
    /// 链走到 init/死根，途中无 installed-app、无存活系统根
    terminates_at_init: bool,
    /// 链上存在「自身已成孤儿」的 shell（死掉的终端会话）
    has_orphan_shell: bool,
    /// 链上存在 pm2 God Daemon
    pm2: bool,
}

pub fn scan(whitelist: &[String]) -> Vec<ProcessEntry> {
    let collected = platform_impl::collect();
    let procs = &collected.procs;

    let mut entries = Vec::new();
    for l in &collected.listeners {
        let meta = procs.get(&l.pid);
        let ppid = meta.map(|m| m.ppid).unwrap_or(0);
        let exe_path = meta.map(|m| m.exe_path.clone()).unwrap_or_default();
        let full_command = meta
            .map(|m| m.full_command.clone())
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| l.command.clone());

        let (app_label, app_category) =
            platform_impl::identify_app(&full_command, &l.command, &exe_path);

        let (parent_chain, chain_flags) = build_parent_chain(l.pid, procs);
        let launcher_label = parent_chain
            .last()
            .map(|p| p.label.clone())
            .unwrap_or_else(|| "?".to_string());

        // —— 豁免规则：installed-app/system 类别豁免；exe 在标准路径也豁免，
        //    但 dev-script 例外 —— 脚本运行时的身份是脚本，不能因解释器
        //    装在系统目录（/usr/bin/python3、Program Files\nodejs）而漏报。
        let exe_is_standard_install = app_category == "installed-app"
            || app_category == "system"
            || (platform_impl::is_standard_install_path(&exe_path) && app_category != "dev-script");

        let snapshot = ProcessSnapshot {
            state: meta.and_then(|m| m.state.clone()),
            elapsed_secs: meta.map(|m| m.elapsed_secs).unwrap_or(0),
            direct_orphan: meta.and_then(|m| platform_impl::direct_orphan(ppid, m, procs)),
            chain_terminates_at_init: chain_flags.terminates_at_init,
            chain_has_orphan_shell: chain_flags.has_orphan_shell,
            launchd_managed: collected.launchd_pids.contains(&l.pid),
            brew_service_path: platform_impl::is_brew_service_path(&exe_path),
            pm2_managed: chain_flags.pm2 || full_command.contains("ProcessContainer"),
            tty_orphaned: meta.map(|m| m.tty_orphaned).unwrap_or(false),
            exe_is_standard_install,
            dev_keyword: is_dev_server(&full_command) || is_dev_server(&l.command),
            dev_category: app_category == "dev-script",
        };
        let verdict = classify(&snapshot);

        // 白名单 key：优先 exe_path（最稳定），否则退回 command
        let wl_key = if !exe_path.is_empty() {
            exe_path.clone()
        } else {
            l.command.clone()
        };
        let is_whitelisted = whitelist.contains(&wl_key);

        let mut ports = l.ports.clone();
        ports.sort_unstable();

        let user = if !l.user.is_empty() {
            l.user.clone()
        } else {
            meta.map(|m| m.user.clone()).unwrap_or_default()
        };

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
            user,
            tty: meta.and_then(|m| m.tty.clone()).unwrap_or_default(),
            elapsed_secs: meta.map(|m| m.elapsed_secs).unwrap_or(0),
            start_unix: meta.and_then(|m| m.start_unix),
            cpu_percent: meta.map(|m| m.cpu_percent).unwrap_or(0.0),
            mem_mb: meta.map(|m| m.rss_kb as f32 / 1024.0).unwrap_or(0.0),
            state: meta.and_then(|m| m.state.clone()).unwrap_or_default(),
            is_zombie_suspect: verdict.is_suspect && !is_whitelisted,
            confidence: verdict.confidence,
            zombie_reasons: verdict.reasons,
            is_whitelisted,
        });
    }

    // 排序：嫌疑优先 → 置信度高优先 → 端口号
    entries.sort_by(|a, b| {
        b.is_zombie_suspect
            .cmp(&a.is_zombie_suspect)
            .then(b.confidence.cmp(&a.confidence))
            .then(
                a.ports
                    .first()
                    .copied()
                    .unwrap_or(0)
                    .cmp(&b.ports.first().copied().unwrap_or(0)),
            )
    });

    entries
}

/// 沿 PPID 向上回溯（≤12 层），同时收集孤儿链信号。
/// 停止条件：init（macOS=launchd，合成根节点）、第一个 installed-app
///（"这个 node 是 iTerm/Cursor 拉起的"）、存活的系统根（Windows explorer 等）、
/// 父缺失（Windows 死根，合成 System 节点）。
fn build_parent_chain(
    start_pid: u32,
    procs: &HashMap<u32, ProcMeta>,
) -> (Vec<ParentRef>, ChainFlags) {
    let mut chain = Vec::new();
    let mut flags = ChainFlags::default();
    let mut current_pid = start_pid;

    // 注：命中 installed-app / 存活系统根即 break，因此走到 init/死根分支时
    // 链上必然没有用户可见 App —— terminates_at_init 直接置 true 即可。
    for _ in 0..12 {
        let Some(current) = procs.get(&current_pid) else {
            break;
        };
        let parent_ppid = current.ppid;

        // init：macOS 走到 launchd
        if platform_impl::chain_hits_init(parent_ppid) {
            chain.push(platform_impl::synth_chain_root());
            flags.terminates_at_init = true;
            break;
        }
        if parent_ppid == 0 || parent_ppid == current_pid {
            // Windows：父未知/已退出 ⇒ 死根；macOS：kernel(0) 处直接收尾
            if cfg!(windows) {
                chain.push(platform_impl::synth_chain_root());
                flags.terminates_at_init = true;
            }
            break;
        }
        let Some(parent) = procs.get(&parent_ppid) else {
            // 父进程已不在快照中：Windows 视为死根；macOS 是快照间隙的瞬态，保守收尾
            if cfg!(windows) {
                chain.push(platform_impl::synth_chain_root());
                flags.terminates_at_init = true;
            }
            break;
        };

        let (label, category) = platform_impl::identify_app(
            &parent.full_command,
            basename(&parent.exe_path),
            &parent.exe_path,
        );

        // 存活的系统根（Windows explorer/services 等）：链的合法终点，非孤儿
        if platform_impl::is_live_session_root(&parent.exe_path) {
            chain.push(ParentRef {
                pid: parent_ppid,
                label,
                category,
                exe_path: parent.exe_path.clone(),
            });
            break;
        }

        // 死掉的终端会话：链上的 shell 自身已是孤儿
        if platform_impl::is_shell(&parent.exe_path)
            && platform_impl::direct_orphan(parent.ppid, parent, procs).is_some()
        {
            flags.has_orphan_shell = true;
        }
        if parent.full_command.contains("PM2") || parent.full_command.contains("God Daemon") {
            flags.pm2 = true;
        }

        let is_user_visible_app = platform_impl::is_chain_stopper(&parent.exe_path, &category);
        chain.push(ParentRef {
            pid: parent_ppid,
            label,
            category,
            exe_path: parent.exe_path.clone(),
        });
        if is_user_visible_app {
            break;
        }
        current_pid = parent_ppid;
    }

    (chain, flags)
}

#[cfg(test)]
mod live_smoke {
    /// 真机冒烟（默认忽略，手动跑：cargo test live_scan -- --ignored --nocapture）：
    /// 对本机真实进程跑一遍完整管道，人工核对分类与豁免是否合理。
    #[test]
    #[ignore]
    fn live_scan() {
        let entries = super::scan(&[]);
        println!("\n==== live scan: {} listeners ====", entries.len());
        for e in &entries {
            println!(
                "{:>6}  :{:<24} {:<14} conf={:<9} reasons={:?}  [{}] {}",
                e.pid,
                e.ports
                    .iter()
                    .map(|p| p.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
                e.app_category,
                format!("{:?}", e.confidence),
                e.zombie_reasons,
                e.app_label,
                e.exe_path
            );
        }
    }
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    fn meta(ppid: u32, exe: &str, cmd: &str) -> ProcMeta {
        ProcMeta {
            ppid,
            exe_path: exe.to_string(),
            full_command: cmd.to_string(),
            user: String::new(),
            start_unix: Some(1000),
            elapsed_secs: 600,
            cpu_percent: 0.0,
            rss_kb: 0,
            tty: None,
            state: None,
            tty_orphaned: false,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn orphan_chain_zsh_npm_vite() {
        // vite(300) ← npm(200) ← zsh(100, ppid=1 已被收养) —— 头号漏报场景
        let mut procs = HashMap::new();
        procs.insert(100, meta(1, "/bin/zsh", "-zsh"));
        procs.insert(200, meta(100, "/opt/homebrew/bin/node", "npm run dev"));
        procs.insert(
            300,
            meta(
                200,
                "/opt/homebrew/bin/node",
                "node /Users/x/proj/node_modules/.bin/vite",
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        assert!(flags.terminates_at_init, "链应终止于 launchd");
        assert!(flags.has_orphan_shell, "链上应识别出孤儿 zsh");
        // 链：npm → zsh → launchd
        assert_eq!(chain.last().unwrap().label, "launchd");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn live_terminal_chain_not_orphan() {
        // vite(300) ← zsh(200) ← Terminal.app(100, 活着)
        let mut procs = HashMap::new();
        procs.insert(
            100,
            meta(
                1,
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            ),
        );
        procs.insert(200, meta(100, "/bin/zsh", "-zsh"));
        procs.insert(
            300,
            meta(
                200,
                "/opt/homebrew/bin/node",
                "node /Users/x/proj/node_modules/.bin/vite",
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        // Terminal.app 虽在 /System/ 下（类别 system），但 is_chain_stopper 按
        // ".app/" 识别为用户可见 App —— 链在此停下，不会误判为孤儿链。
        assert!(!flags.terminates_at_init, "活终端必须挡住孤儿链判定");
        assert!(!flags.has_orphan_shell);
        assert_eq!(chain.last().unwrap().label, "Terminal");
    }
}
