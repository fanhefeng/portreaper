//! 父链回溯：沿 PPID 向上走查，产出启动链（前端 LauncherChain）与孤儿链信号。
//! 从 mod.rs 拆出的独立职责 —— 只依赖进程表与平台叶子的链语义钩子，
//! 不触碰「组装 snapshot」（那是 mod.rs build_entry 的本体）。

use std::collections::HashMap;

use super::identify::AppIdentity;
use super::model::{ParentRef, ProcMeta};
use super::platform_impl;

/// 父链回溯的同时收集的孤儿信号。
#[derive(Default)]
pub(super) struct ChainFlags {
    /// 链走到 init/死根，途中无 installed-app、无存活系统根
    pub(super) terminates_at_init: bool,
    /// 链上存在「自身已成孤儿」的 shell（死掉的终端会话）
    pub(super) has_orphan_shell: bool,
    /// 链上存在 pm2 God Daemon
    pub(super) pm2: bool,
    /// 链在终止前是否走过至少一个**真实**祖先（合成根 synth_chain_root 不算）。
    ///
    /// 为 false 时，「链终止于 init/死根」这件事完全由直接孤儿信号决定、不含任何
    /// 新信息：macOS 的 ppid==1 与 Windows 的 ppid==0 / 父不在表中，都在本函数
    /// 第一次迭代就命中终止分支 —— 而那三种情况恰好也正是两个平台的 direct_orphan
    /// 的全部触发条件。此时 OrphanedChain 只是把 Ppid1Orphan / ParentExited
    /// 换了句话再说一遍（评审发现：按 ReasonCode 变体特判会漏掉 Windows 这一半）。
    pub(super) walked_real_ancestor: bool,
}

/// pm2 托管识别 —— 用「双标记并存」收紧裸子串误命中（评审发现）：单凭整行
/// 含 "PM2"（如 Java 类名 com.foo.PM2Handler）或目录恰名为 "ProcessContainer"
/// 就硬豁免，会让真孤儿漏报。pm2 实际形态唯一性足够：
///   God Daemon 标题恒为 `PM2 vX.Y.Z: God Daemon (...)`（两标记并存）；
///   被托管进程的包装器路径含 `.../pm2/.../ProcessContainer*`（pm2 + 容器名并存）。
pub(super) fn is_pm2_god_daemon(cmd: &str) -> bool {
    cmd.contains("PM2") && cmd.contains("God Daemon")
}
pub(super) fn is_pm2_container(cmd: &str) -> bool {
    cmd.contains("ProcessContainer") && cmd.contains("pm2")
}

/// 沿 PPID 向上回溯（≤12 层），同时收集孤儿链信号。
/// 停止条件：init（macOS=launchd，合成根节点）、第一个 installed-app
///（"这个 node 是 iTerm/Cursor 拉起的"）、存活的系统根（Windows explorer 等）、
/// 父缺失（Windows 死根，合成 System 节点）。
pub(super) fn build_parent_chain(
    start_pid: u32,
    procs: &HashMap<u32, ProcMeta>,
) -> (Vec<ParentRef>, ChainFlags) {
    let mut chain = Vec::new();
    let mut flags = ChainFlags::default();
    let mut current_pid = start_pid;

    // 注：命中 installed-app / 存活系统根即 break，因此走到 init/死根分支时
    // 链上必然没有用户可见 App —— terminates_at_init 直接置 true 即可。

    // 死根收尾（两处共用）。「死根是否算链到 init」是平台语义，由叶子文件的
    // 同签名钩子 dead_root_terminates_chain 决定（Windows true / macOS false，
    // 各自的取舍理由写在钩子旁）—— 编排层不再内嵌 cfg!(windows) 分支，平台
    // 判定 100% 收敛在 macos.rs / windows.rs（与 chain_hits_init /
    // is_live_session_root 同一套架构）。
    fn dead_root(chain: &mut Vec<ParentRef>, flags: &mut ChainFlags) {
        if platform_impl::dead_root_terminates_chain() {
            chain.push(platform_impl::synth_chain_root());
            flags.terminates_at_init = true;
        }
    }

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
            // 父未知/已退出（Windows）或走到 kernel(0)（macOS）
            dead_root(&mut chain, &mut flags);
            break;
        }
        let Some(parent) = procs.get(&parent_ppid) else {
            // 父进程已不在快照中
            dead_root(&mut chain, &mut flags);
            break;
        };

        let AppIdentity { label, category } = platform_impl::identify_app(
            &parent.full_command,
            super::identify::basename(&parent.exe_path),
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
            flags.walked_real_ancestor = true;
            break;
        }

        // 死掉的终端会话：链上的 shell 自身已是孤儿
        if platform_impl::is_shell(&parent.exe_path)
            && platform_impl::direct_orphan(parent.ppid, parent, procs).is_some()
        {
            flags.has_orphan_shell = true;
        }
        if is_pm2_god_daemon(&parent.full_command) {
            flags.pm2 = true;
        }

        let is_user_visible_app = platform_impl::is_chain_stopper(&parent.exe_path, &category);
        chain.push(ParentRef {
            pid: parent_ppid,
            label,
            category,
            exe_path: parent.exe_path.clone(),
        });
        flags.walked_real_ancestor = true;
        if is_user_visible_app {
            break;
        }
        current_pid = parent_ppid;
    }

    (chain, flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pm2_detection_requires_both_markers() {
        // 真实 pm2 形态命中
        assert!(is_pm2_god_daemon("PM2 v6.0.5: God Daemon (/Users/x/.pm2)"));
        assert!(is_pm2_container(
            "node /usr/local/lib/node_modules/pm2/lib/ProcessContainerFork.js"
        ));
        // 误命中面：单标记不豁免（评审发现）
        assert!(!is_pm2_god_daemon("java -cp app.jar com.foo.PM2Handler"));
        assert!(!is_pm2_god_daemon("node /Users/x/God Daemon Sim/server.js"));
        assert!(!is_pm2_container("node /Users/x/ProcessContainer/index.js"));
    }
}

#[cfg(all(test, target_os = "macos"))] // 链 fixture 全部基于 macOS 进程形态
mod macos_chain_tests {
    use super::super::model::ProcMeta;
    use super::*;

    #[test]
    fn orphan_chain_zsh_npm_vite() {
        // vite(300) ← npm(200) ← zsh(100, ppid=1 已被收养) —— 头号漏报场景
        let mut procs = HashMap::new();
        procs.insert(100, ProcMeta::fixture(1, "/bin/zsh", "-zsh"));
        procs.insert(
            200,
            ProcMeta::fixture(100, "/opt/homebrew/bin/node", "npm run dev"),
        );
        procs.insert(
            300,
            ProcMeta::fixture(
                200,
                "/opt/homebrew/bin/node",
                "node /Users/x/proj/node_modules/.bin/vite",
            ),
        );

        let (chain, flags) = build_parent_chain(300, &procs);
        assert!(flags.terminates_at_init, "链应终止于 launchd");
        assert!(flags.has_orphan_shell, "链上应识别出孤儿 zsh");
        assert!(
            flags.walked_real_ancestor,
            "走过 npm、zsh 才撞到 launchd ⇒ 链是一份独立证据"
        );
        // 链：npm → zsh → launchd
        assert_eq!(chain.last().unwrap().label, "launchd");
    }

    /// 锁住 classify 的 OrphanedChain 去重所依赖的**前提**（评审发现：此前只有
    /// 纯函数侧断言了结论，没有任何测试 pin 住产生该前提的这次遍历）。
    ///
    /// 前提是：本体 ppid==1 时，遍历从自己起步、第一次迭代就命中 chain_hits_init，
    /// 一个真实祖先都没走过。若将来有人把起点改成 meta.ppid、或在 chain_hits_init
    /// 之前插入别的终止分支，这条断言会先红 —— 否则 classify 会继续默默吞掉一条
    /// 此时已经变得独立的 OrphanedChain 证据，而全部纯函数测试照样全绿。
    #[test]
    fn ppid1_leaf_terminates_before_walking_any_ancestor() {
        let mut procs = HashMap::new();
        procs.insert(
            400,
            ProcMeta::fixture(1, "/opt/homebrew/bin/node", "node /Users/x/proj/server.js"),
        );
        let (chain, flags) = build_parent_chain(400, &procs);
        assert!(flags.terminates_at_init, "ppid=1 ⇒ 链终止于 launchd");
        assert!(
            !flags.walked_real_ancestor,
            "ppid=1 时第一次迭代即终止，不得走过任何真实祖先"
        );
        assert_eq!(chain.len(), 1, "链上只有合成的 launchd 根");
        assert_eq!(chain[0].label, "launchd");
    }

    #[test]
    fn live_terminal_chain_not_orphan() {
        // vite(300) ← zsh(200) ← Terminal.app(100, 活着)
        let mut procs = HashMap::new();
        procs.insert(
            100,
            ProcMeta::fixture(
                1,
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
                "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            ),
        );
        procs.insert(200, ProcMeta::fixture(100, "/bin/zsh", "-zsh"));
        procs.insert(
            300,
            ProcMeta::fixture(
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
