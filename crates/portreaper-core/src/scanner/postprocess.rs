//! 跨条目后处理：单进程纯函数（classify）看不到的全局信号在此补齐 ——
//! 同项目重复 dev server 标注（mark_duplicates）与展示用的子树 CPU 合计
//! （fill_subtree_cpu）。两者都只改写已产出的 ProcessEntry，不回头影响判定。

use std::collections::{HashMap, HashSet};

use super::classify::{Confidence, ReasonCode};
use super::identify::{self, basename};
use super::model::{ProcMeta, ProcessEntry};
use super::platform_impl;

/// 同项目重复 dev server 检测（跨条目后处理）。覆盖两类真实场景：
///   a) 完整命令逐字相同 —— 忘了已启动过，又跑了一遍同一条命令
///     （vite 会把第二个实例顺延到 3001，端口被白占）；
///   b) 同项目在不同终端 / IDE（Warp 起 5173、VS Code 起 5174）各起了一个实例
///     —— cwd 相同 + 脚本/模块身份相同，或路径推断的（项目, 脚本）一致。
///
/// cwd 是最强证据（评审发现）：monorepo 各子包 / git worktree 的 cwd 必然不同
/// （turbo 等编排器按包目录设 cwd），同项目重复启动的 cwd 必然相同 ——
/// 两侧 cwd 已知且不同 ⇒ 一票否决（hoisted node_modules 让路径推断的项目名
/// 全部坍缩到仓库根，仅靠路径会把 monorepo 的两个 app 误判成重复）。
///
/// 其余排除（全部有测试锁定）：
///   - 端口集相交：SO_REUSEPORT 多 worker 共享端口，不是重复；
///   - 互为父子：cluster master/worker；
///   - 真实存活的同一非 shell 父（concurrently / cluster master）⇒ 有意多实例；
///     父是 shell 或已死（合成 init 根）则照常比对 —— 同一终端重复跑两次、
///     双双被收养的孤儿对，正是要抓的场景（评审发现）；
///   - 共同祖父且祖父是存活的非 shell 编排器（turbo 经 shell 包装的堂兄弟）。
///     编排器证据**排除用户可见 App**（is_chain_stopper）：同一个 Terminal/iTerm
///     的两个 tab 各起一个 vite，共同祖父是终端 App 进程 —— 终端不是编排器，
///     tab 是独立会话，这正是要抓的重复（评审发现：终端祖父曾被误当 turbo 豁免）。
///
/// 不变量：重复信号只到 Possible，永不入清扫 —— 机器无法判断用户正在用哪个实例。
pub(super) fn mark_duplicates(entries: &mut [ProcessEntry], cwds: &HashMap<u32, String>) {
    fn eligible(e: &ProcessEntry) -> bool {
        e.app_category == super::DEV_SCRIPT_CATEGORY
            && !e.is_whitelisted
            // 被硬豁免的条目不参与：非嫌疑但带豁免原因（launchd/brew/pm2/标准路径）
            && (e.is_zombie_suspect || e.zombie_reasons.is_empty())
    }
    /// 脚本/模块身份：vite.js / http.server（b 档比对的一半）
    fn script_identity(e: &ProcessEntry) -> Option<String> {
        identify::extract_script_arg(&e.full_command)
            .map(|s| basename(s).to_string())
            .or_else(|| identify::extract_module_arg(&e.full_command).map(String::from))
    }
    /// 路径推断身份键：（项目名, 脚本）—— cwd 不可用时的回退
    fn project_key(e: &ProcessEntry) -> Option<(String, String)> {
        Some((
            identify::extract_project_name(&e.full_command)?,
            script_identity(e)?,
        ))
    }
    /// (pid, is_shell, is_user_visible_app)：后两者都排除「编排器」资格 ——
    /// shell 只是包装、用户可见 App（终端/IDE）不是编排器。
    fn chain_node(e: &ProcessEntry, depth: usize) -> Option<(u32, bool, bool)> {
        e.parent_chain.get(depth).map(|p| {
            (
                p.pid,
                platform_impl::is_shell(&p.exe_path),
                platform_impl::is_chain_stopper(&p.exe_path, &p.category),
            )
        })
    }
    /// peer 选取确定化（评审 E1 补全）：mark_duplicates 跑在 HashMap 迭代序上，
    /// ≥3 个重复实例时 get_or_insert 的「第一个匹配」会随轮询随机翻转，前端
    /// 「与 PID X 重复」闪变 —— 恒取最小 PID peer，与遍历顺序无关。
    fn assign_min(slot: &mut Option<u32>, pid: u32) {
        *slot = Some(slot.map_or(pid, |cur| cur.min(pid)));
    }

    // 预计算每个条目的派生身份:原实现在内层循环里对固定的 a 反复重算
    // project_key / script_identity(各含一次 split_whitespace 全命令行扫描)
    // 与 chain_node,是 O(n²)×解析。这里每条目只算一次,内层只做比较(评审 H1)。
    // 不 eligible 的条目留空,内层据 eligible 直接跳过。
    #[derive(Default)]
    struct Prep {
        eligible: bool,
        project: Option<(String, String)>,
        script_id: Option<String>,
        cwd: Option<String>,
        chain0: Option<(u32, bool, bool)>,
        chain1: Option<(u32, bool, bool)>,
    }
    let prep: Vec<Prep> = entries
        .iter()
        .map(|e| {
            if !eligible(e) {
                return Prep::default();
            }
            Prep {
                eligible: true,
                project: project_key(e),
                script_id: script_identity(e),
                cwd: cwds.get(&e.pid).cloned(),
                chain0: chain_node(e, 0),
                chain1: chain_node(e, 1),
            }
        })
        .collect();

    let n = entries.len();
    let mut peer: Vec<Option<u32>> = vec![None; n];
    for i in 0..n {
        if !prep[i].eligible {
            continue;
        }
        for j in (i + 1)..n {
            if !prep[j].eligible {
                continue;
            }
            let (a, b) = (&entries[i], &entries[j]);
            let (pi, pj) = (&prep[i], &prep[j]);
            // 互为父子（master/worker）
            if a.ppid == b.pid || b.ppid == a.pid {
                continue;
            }
            // 端口集相交（多 worker 共享端口）
            if a.ports.iter().any(|p| b.ports.contains(p)) {
                continue;
            }
            // cwd 一票否决：两侧已知且不同 ⇒ 不同子包/worktree/项目
            let (cwd_a, cwd_b) = (pi.cwd.as_deref(), pj.cwd.as_deref());
            if let (Some(ca), Some(cb)) = (cwd_a, cwd_b) {
                if ca != cb {
                    continue;
                }
            }
            // 真实存活的同一非 shell、非用户可见 App 父 ⇒ 编排器拉起的有意多实例；
            // 父是 shell / 用户可见 App / 已死（合成根 pid≤1）/ 链缺失 ⇒ 照常比对。
            // 两侧链都须独立印证该父（评审发现：只验 a 一侧、默认 b 链一致 ——
            // b 链缺失/形态不同时会被静默当作同编排器而漏标；收紧到双侧印证）。
            if a.ppid == b.ppid {
                if let (Some((pa, pa_sh, pa_app)), Some((pb, _, _))) = (pi.chain0, pj.chain0) {
                    if pa == a.ppid && pb == b.ppid && pa > 1 && !pa_sh && !pa_app {
                        continue;
                    }
                }
            }
            // 共同祖父的堂兄弟：祖父是存活的非 shell 编排器（turbo 经 shell 包装）。
            // 两侧 is_shell 都须为假（pid 相等时本是同进程、冗余但语义自证）；
            // 祖父是用户可见 App（同一终端两个 tab）不构成编排证据，照常比对。
            if let (Some((ga, ga_sh, ga_app)), Some((gb, gb_sh, _))) = (pi.chain1, pj.chain1) {
                if ga == gb && ga > 1 && !ga_sh && !gb_sh && !ga_app {
                    continue;
                }
            }
            let same_cmd = !a.full_command.is_empty() && a.full_command == b.full_command;
            let same_project = match (&pi.project, &pj.project) {
                (Some(x), Some(y)) => x == y,
                _ => false,
            };
            let same_cwd = matches!((cwd_a, cwd_b), (Some(x), Some(y)) if x == y);
            let same_cwd_identity = same_cwd
                && matches!(
                    (&pi.script_id, &pj.script_id),
                    (Some(x), Some(y)) if x == y
                );
            // 路径/命令证据（same_cmd / same_project）在 hoisted node_modules 下会把
            // 不同 monorepo 子包坍缩成相同 —— 仅当 cwd 信息不是「一侧已知、一侧未知」
            // 时才采信（评审 M3：信息不对称时已知侧无从印证未知侧，易把子包误配对）。
            // 两侧都未知是纯路径回退（接受其风险）；两侧都已知到此必然相同（不同已被
            // 上面一票否决）。same_cwd_identity 本就要求双 cwd 相同，不受此限。
            let cwd_known = cwd_a.is_some() as u8 + cwd_b.is_some() as u8;
            let path_evidence_ok = cwd_known != 1;
            if same_cwd_identity || ((same_cmd || same_project) && path_evidence_ok) {
                assign_min(&mut peer[i], b.pid);
                assign_min(&mut peer[j], a.pid);
            }
        }
    }
    for (i, p) in peer.into_iter().enumerate() {
        let Some(pid) = p else { continue };
        let e = &mut entries[i];
        e.duplicate_of = Some(pid);
        e.zombie_reasons.push(ReasonCode::DuplicateDevServer);
        if !e.is_zombie_suspect {
            e.is_zombie_suspect = true;
            e.confidence = Confidence::Possible;
        }
    }
}

/// 子树 CPU 合计（展示用后处理）：把「自身 + 全部后代」的 pcpu 累加到被列出的那行。
///
/// 为什么必要（KNOWN-GAPS Gap 1/B 的真实荒诞点）：headless 浏览器主进程显示 ~0%，
/// 而它子树里的 `--type=gpu-process` 在满核空转 —— 只看行内 CPU，用户与判定链路
/// 都完全看不出异常。数据源是已采集的全进程表（ppid + cpu_percent），纯内存聚合、
/// 零额外系统调用；**不进 ProcessSnapshot、不参与判定**（健康的 vite build 一样满核）。
///
/// 一次构建父子索引后逐行 DFS：行数（几十）× 深度，远小于逐行全表扫描。
/// visited 兜住进程表快照里可能出现的自环 / 环路（父子创建瞬间的 ppid 竞态），
/// 否则 DFS 会死循环、整次扫描挂住（前端表现为 ERR_SCAN_TIMEOUT）。
pub(super) fn fill_subtree_cpu(entries: &mut [ProcessEntry], procs: &HashMap<u32, ProcMeta>) {
    if entries.is_empty() {
        return;
    }
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, meta) in procs {
        if meta.ppid != pid {
            children.entry(meta.ppid).or_default().push(pid);
        }
    }
    let mut visited: HashSet<u32> = HashSet::new();
    let mut stack: Vec<u32> = Vec::new();
    for e in entries.iter_mut() {
        visited.clear();
        stack.clear();
        // 根节点取行自身的值（entry 的 cpu_percent 就是它）—— 进程表里查不到自己
        // （两次快照的间隙）时优雅退化为自身 CPU，而不是把行清零。
        let mut total = e.cpu_percent;
        visited.insert(e.pid);
        stack.push(e.pid);
        while let Some(pid) = stack.pop() {
            let Some(kids) = children.get(&pid) else {
                continue;
            };
            for &kid in kids {
                if !visited.insert(kid) {
                    continue; // 环 / 重复入栈：每个节点只计一次
                }
                if let Some(meta) = procs.get(&kid) {
                    total += meta.cpu_percent;
                }
                stack.push(kid);
            }
        }
        e.cpu_percent_tree = total;
    }
}

#[cfg(test)] // 平台中性：ProcessEntry 纯数据，is_shell 用 bash（双平台 shell 表都含）
mod tests {
    use super::super::model::ParentRef;
    use super::*;

    const VITE_A: &str = "node /Users/x/ai-portal/node_modules/vite/bin/vite.js dev --port 3000";

    fn entry(pid: u32, ppid: u32, ports: &[u16], cmd: &str) -> ProcessEntry {
        ProcessEntry {
            pid,
            ppid,
            ports: ports.to_vec(),
            command: "node".into(),
            full_command: cmd.into(),
            exe_path: "/opt/homebrew/bin/node".into(),
            app_label: String::new(),
            app_category: "dev-script".into(),
            parent_chain: vec![],
            launcher_label: String::new(),
            user: String::new(),
            tty: String::new(),
            elapsed_secs: 3600,
            start_unix: Some(1000),
            cpu_percent: 0.0,
            mem_mb: 0.0,
            state: String::new(),
            is_zombie_suspect: false,
            confidence: Confidence::None,
            zombie_reasons: vec![],
            is_whitelisted: false,
            // 夹具用固定 exe 路径，与 build_entry 的推导一致（含分隔符 ⇒ 用 exe_path）
            whitelist_key: "/opt/homebrew/bin/node".into(),
            duplicate_of: None,
            cpu_percent_tree: 0.0,
        }
    }

    fn parent(pid: u32, exe: &str) -> ParentRef {
        ParentRef {
            pid,
            label: basename(exe).to_string(),
            category: "unknown".into(),
            exe_path: exe.into(),
        }
    }

    fn no_cwd() -> HashMap<u32, String> {
        HashMap::new()
    }

    fn cwd_map(pairs: &[(u32, &str)]) -> HashMap<u32, String> {
        pairs.iter().map(|(p, c)| (*p, c.to_string())).collect()
    }

    #[test]
    fn exact_command_duplicate_flagged_both_ways() {
        // ai-portal 真实案例：同命令、同 cwd、不同父链、端口被顺延
        let mut a = entry(88898, 88877, &[3000, 4206], VITE_A);
        a.parent_chain = vec![
            parent(88877, "/opt/homebrew/bin/node"),
            parent(88876, "/usr/local/bin/node"),
        ];
        let mut b = entry(46392, 46371, &[3001, 61405], VITE_A);
        b.parent_chain = vec![
            parent(46371, "/opt/homebrew/bin/node"),
            parent(46370, "/usr/local/bin/node"),
        ];
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[(88898, "/Users/x/ai-portal"), (46392, "/Users/x/ai-portal")]),
        );
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
        assert_eq!(es[0].confidence, Confidence::Possible);
        assert_eq!(es[0].duplicate_of, Some(46392));
        assert_eq!(es[1].duplicate_of, Some(88898));
        assert!(es[0]
            .zombie_reasons
            .contains(&ReasonCode::DuplicateDevServer));
    }

    #[test]
    fn same_project_different_launcher_flagged() {
        // 用户场景：Warp 起 5173、VS Code 起 5174 —— 命令参数不同但项目+脚本一致
        let a = entry(100, 10, &[5173], VITE_A);
        let b = entry(
            200,
            20,
            &[5174],
            "node /Users/x/ai-portal/node_modules/vite/bin/vite.js dev",
        );
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
        assert_eq!(es[0].duplicate_of, Some(200));
    }

    #[test]
    fn same_cwd_identity_flagged_outside_users() {
        // 项目不在 /Users 下（路径推断失效）：cwd + 脚本身份兜底
        let a = entry(100, 10, &[8080], "node /srv/proj/server.js");
        let b = entry(200, 20, &[8081], "node /srv/proj/server.js --verbose");
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &cwd_map(&[(100, "/srv/proj"), (200, "/srv/proj")]));
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn monorepo_apps_different_cwd_not_flagged() {
        // 评审发现：hoisted node_modules 下两个不同 app 的命令行可能逐字相同，
        // 路径推断的项目名也坍缩到仓库根 —— cwd 不同一票否决
        let cmd = "node /Users/x/mono/node_modules/vite/bin/vite.js dev";
        let a = entry(100, 10, &[3000], cmd);
        let b = entry(200, 20, &[3001], cmd);
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[
                (100, "/Users/x/mono/apps/web"),
                (200, "/Users/x/mono/apps/docs"),
            ]),
        );
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn worktrees_different_cwd_not_flagged() {
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(
            &mut es,
            &cwd_map(&[(100, "/Users/x/ai-portal"), (200, "/Users/x/ai-portal-wt2")]),
        );
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn asymmetric_cwd_path_evidence_not_flagged() {
        // 评审 M3:cwd「一侧已知、一侧未知」时,路径/命令证据(hoisted node_modules
        // 会把不同 monorepo 子包坍缩成逐字相同)不可信 —— 已知侧无从印证未知侧,
        // 不据此标重复,避免把子包误配对。两侧都未知(no_cwd)仍走纯路径回退。
        let cmd = "node /Users/x/mono/node_modules/vite/bin/vite.js dev";
        let a = entry(100, 10, &[3000], cmd);
        let b = entry(200, 20, &[3001], cmd);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &cwd_map(&[(100, "/Users/x/mono/apps/web")]));
        assert!(
            !es[0].is_zombie_suspect && !es[1].is_zombie_suspect,
            "cwd 信息不对称时路径证据不可信,不应标重复"
        );
    }

    #[test]
    fn cluster_workers_same_master_not_flagged() {
        // 同父且父是存活的编排器（node master）：有意的多实例
        let mut a = entry(100, 600, &[3000], VITE_A);
        a.parent_chain = vec![parent(600, "/opt/homebrew/bin/node")];
        let mut b = entry(200, 600, &[3001], VITE_A);
        b.parent_chain = vec![parent(600, "/opt/homebrew/bin/node")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn same_shell_run_twice_flagged() {
        // 同一个 shell 里后台跑了两次：正是要抓的重复
        let mut a = entry(100, 500, &[3000], VITE_A);
        a.parent_chain = vec![parent(500, "/bin/bash")];
        let mut b = entry(200, 500, &[3001], VITE_A);
        b.parent_chain = vec![parent(500, "/bin/bash")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn coreparented_orphan_siblings_flagged() {
        // 评审发现：双双被收养（ppid=1，链上是合成 init 根）的同命令对
        // 不能被「同父编排器」守卫吞掉 —— 父已死不构成编排证据
        let mut a = entry(100, 1, &[3000], VITE_A);
        a.parent_chain = vec![parent(1, "/sbin/launchd")];
        let mut b = entry(200, 1, &[3001], VITE_A);
        b.parent_chain = vec![parent(1, "/sbin/launchd")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(es[0].is_zombie_suspect && es[1].is_zombie_suspect);
    }

    #[test]
    fn orchestrator_cousins_not_flagged() {
        // turbo 经 shell 包装拉起的堂兄弟：共同祖父是存活的编排器（非 shell）
        let mut a = entry(100, 601, &[3000], VITE_A);
        a.parent_chain = vec![parent(601, "/bin/sh"), parent(700, "/usr/local/bin/node")];
        let mut b = entry(200, 602, &[3001], VITE_A);
        b.parent_chain = vec![parent(602, "/bin/sh"), parent(700, "/usr/local/bin/node")];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn terminal_app_grandparent_does_not_exempt() {
        // 评审发现：同一个 Terminal.app 的两个 tab 各直接 exec 了一遍 vite ——
        // 共同祖父是终端 App 进程（存活、非 shell），但终端不是编排器，
        // tab 是独立会话，必须照常标重复。用户可见 App（is_chain_stopper）
        // 不构成编排证据。category 用 installed-app 使双平台判定一致。
        fn term_parent(pid: u32) -> ParentRef {
            ParentRef {
                pid,
                label: "iTerm2".into(),
                category: "installed-app".into(),
                exe_path: "/Applications/iTerm.app/Contents/MacOS/iTerm2".into(),
            }
        }
        let mut a = entry(100, 501, &[5173], VITE_A);
        a.parent_chain = vec![parent(501, "/bin/zsh"), term_parent(900)];
        let mut b = entry(200, 502, &[5174], VITE_A);
        b.parent_chain = vec![parent(502, "/bin/zsh"), term_parent(900)];
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(
            es[0].is_zombie_suspect && es[1].is_zombie_suspect,
            "终端 App 祖父不得被当作编排器豁免"
        );
        assert_eq!(es[0].confidence, Confidence::Possible);
    }

    #[test]
    fn peer_selection_deterministic_with_three_instances() {
        // 评审 E1 补全：≥3 个重复实例时 peer 必须与遍历顺序无关 ——
        // 恒为「除自己外的最小 PID」。两种入参顺序断言同一结果。
        for order in [[300u32, 100, 200], [100, 200, 300]] {
            let mut es: Vec<ProcessEntry> = order
                .iter()
                .map(|&pid| entry(pid, pid + 1000, &[(pid / 100) as u16 + 3000], VITE_A))
                .collect();
            mark_duplicates(&mut es, &no_cwd());
            for e in &es {
                let want = if e.pid == 100 { 200 } else { 100 };
                assert_eq!(
                    e.duplicate_of,
                    Some(want),
                    "pid {} 的 peer 应恒为最小对端（入参顺序 {:?}）",
                    e.pid,
                    order
                );
            }
        }
    }

    #[test]
    fn shared_port_workers_not_flagged() {
        // SO_REUSEPORT 多 worker 共享端口
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 20, &[3000, 3005], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn parent_child_not_flagged() {
        let a = entry(100, 10, &[3000], VITE_A);
        let b = entry(200, 100, &[3001], VITE_A); // b 是 a 的子进程
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn different_projects_same_script_not_flagged() {
        let a = entry(
            100,
            10,
            &[3000],
            "node /Users/x/blog/node_modules/vite/bin/vite.js dev",
        );
        let b = entry(
            200,
            20,
            &[3001],
            "node /Users/x/shop/node_modules/vite/bin/vite.js dev",
        );
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    #[test]
    fn whitelisted_excluded() {
        let mut a = entry(100, 10, &[3000], VITE_A);
        a.is_whitelisted = true;
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert!(!es[0].is_zombie_suspect && !es[1].is_zombie_suspect);
    }

    /// 展示用的子树 CPU 聚合（KNOWN-GAPS Gap 1/B）：被列出的主进程行显示 ~0%，
    /// 而它的 gpu-process 子进程在满核 —— 合计必须落到那一行上，否则用户与
    /// 判定链路都看不出「空转 7 小时的进程树」和一个闲置进程有什么区别。
    /// 同时锁死环路防护：ppid 竞态造成的环不得让 DFS 死循环（会挂住整次扫描）。
    #[test]
    fn subtree_cpu_aggregates_children_and_survives_cycles() {
        fn proc(ppid: u32, cpu: f32) -> ProcMeta {
            let mut m = ProcMeta::fixture(ppid, "", "");
            m.cpu_percent = cpu;
            m
        }
        let mut procs = HashMap::new();
        procs.insert(100, proc(1, 0.4)); // headless 主进程：行内看着是闲的
        procs.insert(101, proc(100, 99.2)); // gpu-process：真凶
        procs.insert(102, proc(101, 1.0)); // 孙节点也要计入
        procs.insert(103, proc(104, 5.0)); // 环：103 ↔ 104
        procs.insert(104, proc(103, 7.0));

        let mut entries = vec![entry(100, 1, &[9339], VITE_A), entry(103, 104, &[], VITE_A)];
        entries[0].cpu_percent = 0.4; // 行自身就是「看着很闲」的那个数
        entries[1].cpu_percent = 5.0;
        fill_subtree_cpu(&mut entries, &procs);
        assert!(
            (entries[0].cpu_percent_tree - 100.6).abs() < 0.01,
            "主进程行应汇总整棵子树，实得 {}",
            entries[0].cpu_percent_tree
        );
        assert!(
            (entries[1].cpu_percent_tree - 12.0).abs() < 0.01,
            "环路必须收敛且每个节点只计一次，实得 {}",
            entries[1].cpu_percent_tree
        );
    }

    /// 进程表里查不到自己（lsof 与 ps 两次快照的间隙）时不得把行清零 ——
    /// 退化为自身 CPU 是最保守的展示语义。
    #[test]
    fn subtree_cpu_handles_missing_process_gracefully() {
        let mut entries = vec![entry(999, 1, &[3000], VITE_A)];
        entries[0].cpu_percent = 3.5;
        fill_subtree_cpu(&mut entries, &HashMap::new());
        assert_eq!(entries[0].cpu_percent_tree, 3.5, "退化为自身 CPU，不清零");
    }

    #[test]
    fn existing_suspect_keeps_confidence_gains_reason() {
        // 一个本来就是孤儿 Confirmed：置信度不降级，只追加重复信号
        let mut a = entry(100, 1, &[3000], VITE_A);
        a.is_zombie_suspect = true;
        a.confidence = Confidence::Confirmed;
        a.zombie_reasons = vec![ReasonCode::Ppid1Orphan];
        let b = entry(200, 20, &[3001], VITE_A);
        let mut es = vec![a, b];
        mark_duplicates(&mut es, &no_cwd());
        assert_eq!(es[0].confidence, Confidence::Confirmed);
        assert!(es[0]
            .zombie_reasons
            .contains(&ReasonCode::DuplicateDevServer));
        assert_eq!(es[0].duplicate_of, Some(200));
        assert_eq!(es[1].confidence, Confidence::Possible);
    }
}
