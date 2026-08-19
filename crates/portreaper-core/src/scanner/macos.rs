//! macOS 数据采集与路径规则。
//! 三个数据源：lsof（监听套接字）、ps（全进程元数据；会话首进程靠 state 的
//! 's' 标志识别）、launchctl（托管 PID 集合）。
//! 全部子进程强制 en_US.UTF-8，避免本地化输出破坏列解析。

use std::collections::{HashMap, HashSet};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::classify::ReasonCode;
use super::identify::{basename, project_binary_label, AppIdentity};
use super::model::{Collected, Listener, ParentRef, ProcMeta};
use super::{
    AUTOMATION_CATEGORY, DEV_SCRIPT_CATEGORY, INSTALLED_APP_CATEGORY, SYSTEM_CATEGORY,
    UNKNOWN_CATEGORY, USER_BINARY_CATEGORY,
};

// 系统组件路径前缀 —— /System、/Library、/usr 等下的可执行文件视为系统组件。
// identify_app 的「系统组件」归类（step 2b）与 is_standard_install_path 的豁免共用
// 这一份，避免两处列表 drift（评审 E2）。/Applications/ 与 /private/var/folders/
// 不在此列：前者归 installed-app（step 2a），后者要让 dev-build 产物先被 step 5 接住。
const SYSTEM_COMPONENT_PREFIXES: &[&str] = &[
    "/System/",
    "/Library/",
    "/usr/libexec/",
    "/usr/sbin/",
    "/usr/bin/",
    "/sbin/",
    "/bin/",
];

/// Homebrew 服务路径 —— brew services 启动的守护（postgres/redis/...）的兜底豁免。
/// launchctl list 只能看到当前用户 GUI domain，system-domain 的服务靠这个路径特征兜底。
const BREW_SERVICE_PREFIXES: &[&str] = &[
    "/opt/homebrew/opt/",
    "/opt/homebrew/Cellar/",
    "/usr/local/opt/",
    "/usr/local/Cellar/",
];

const SHELLS: &[&str] = &["zsh", "bash", "sh", "fish", "dash", "csh", "tcsh"];

pub(crate) fn is_standard_install_path(exe_path: &str) -> bool {
    // 豁免全集 = 系统组件 + /Applications/（installed-app）+ App Translocation 临时目录
    exe_path.starts_with("/Applications/")
        || exe_path.starts_with("/private/var/folders/")
        || SYSTEM_COMPONENT_PREFIXES
            .iter()
            .any(|p| exe_path.starts_with(p))
}

/// 「这个 exe 确实装在常规安装位置吗」—— **陈述事实**，供 NonstandardPath 那条
/// 说给用户听的理由取证。刻意与 `is_standard_install_path` 分开：那个是**豁免策略**，
/// 会刻意向 true 偏（它把 `/private/var/folders/` 也算进去，为 App Translocation
/// 让路），而临时目录恰恰是「非常规安装位置」最成立的场景 —— `go run` 的临时编译
/// 产物就住在那里，识别为 dev-script 后正需要这条证据。
///
/// 与 identify.rs `is_temp_dir_path` 同源的教训（那里的注释写着「两者语义相反，
/// 绝不能共用同一个函数」）：豁免谓词的每一次放宽都是为了少杀人，拿它陈述事实，
/// 放宽一次就多撒一次谎。
pub(crate) fn is_conventional_install_path(exe_path: &str) -> bool {
    exe_path.starts_with("/Applications/")
        || SYSTEM_COMPONENT_PREFIXES
            .iter()
            .any(|p| exe_path.starts_with(p))
}

pub(crate) fn is_brew_service_path(exe_path: &str) -> bool {
    BREW_SERVICE_PREFIXES
        .iter()
        .any(|p| exe_path.starts_with(p))
}

pub(crate) fn is_shell(exe_path: &str) -> bool {
    let name = basename(exe_path).trim_start_matches('-');
    SHELLS.contains(&name)
}

/// 直接孤儿：macOS 上孤儿会被立即收养到 launchd（PID 1）。
pub(crate) fn direct_orphan(
    ppid: u32,
    _meta: &ProcMeta,
    _procs: &HashMap<u32, ProcMeta>,
) -> Option<ReasonCode> {
    (ppid == 1).then_some(ReasonCode::Ppid1Orphan)
}

/// 父链根的合成节点：链走到 PID 1 时挂一个 launchd 节点。
pub(crate) fn synth_chain_root() -> ParentRef {
    ParentRef {
        pid: 1,
        label: "launchd".to_string(),
        category: SYSTEM_CATEGORY.to_string(),
        exe_path: "/sbin/launchd".to_string(),
    }
}

/// 链走到 PID 1 即 init；macOS 没有 Windows 那种「存活系统根」概念。
pub(crate) fn chain_hits_init(parent_ppid: u32) -> bool {
    parent_ppid == 1
}

/// 链走到死根（ppid==0 / 父不在快照）时是否算「链到 init」——**否**。
/// macOS 上这只是 kernel(0) 或两次快照间隙的瞬态，保守收尾、不下结论；
/// 真正的 init 终点由 `chain_hits_init`（PID 1 = launchd）表达。
/// 与 windows.rs 的同签名钩子成对（那边为 true）—— 平台语义 100% 收敛在
/// 叶子文件，编排层（chain.rs）不再内嵌 `cfg!(windows)`。
pub(crate) fn dead_root_terminates_chain() -> bool {
    false
}

pub(crate) fn is_live_session_root(_exe_path: &str) -> bool {
    false
}

/// 链回溯的「用户可见 App」终点：installed-app 之外还包括任何 .app bundle ——
/// 系统自带 Terminal.app 位于 /System/Applications/（类别 system），
/// 若不在它处停下，链会一路走到 launchd，把活终端里的 dev server 误报成孤儿链。
///
/// `.app/` 兜底**刻意不看 category**，即使 dev 工具自带的运行时
/// （node_modules/electron/dist/Electron.app、ms-playwright 的 Chromium.app，
/// 类别 dev-script / automation-instance）也照停 —— 见
/// `chain_stopper_stops_at_dev_runtimes_on_purpose` 的理由。
pub(crate) fn is_chain_stopper(exe_path: &str, category: &str) -> bool {
    category == INSTALLED_APP_CATEGORY || exe_path.contains(".app/")
}

/// macOS 路径阶梯。顺序敏感：脚本/模块身份 → 自动化实例 →
/// 非 .app dev 运行时 → .app → /Applications 裸 → 系统 → 裸脚本运行时 →
/// Homebrew CLI → cargo 产物 → 用户目录 → unknown。脚本/模块必须最先判：
/// 解释器自身可能就住在 .app bundle / 系统路径里（Python.app、/usr/bin/python3）。
pub(crate) fn identify_app(full_command: &str, short_command: &str, exe_path: &str) -> AppIdentity {
    let exe = exe_path;

    // 0. 脚本/模块身份优先于一切路径与 .app 判定 —— 决策树共享在
    //    identify::script_identity_step（双平台逐行同构，曾各写一份且真漂移过）。
    //    macOS 侧注入的差异：标签用原样 short_command；脚本自身也在常规安装路径时
    //    归「系统自带的脚本任务」（/System/.../foo.py → system）。
    //
    //    这里必须用**事实**谓词 is_conventional_install_path，不能用豁免谓词
    //    is_standard_install_path（评审发现）：后者刻意把 /private/var/folders/ 也算
    //    进去，那条 carve-out 是给 App Translocation 的 .app 让路的，而 .app 要到
    //    阶梯 1 才处理。用在这里的后果是 `python3 /private/var/folders/…/serve.py`
    //    被判成「系统自带脚本」→ 类别 system → exe_is_standard_install 为真 →
    //    classify 吃下 InstalledApp 硬豁免，永不标记；无端口时整行都不出现。
    //    安装器/测试框架把脚本落在 TMPDIR 是常见形态，而临时目录恰恰是「非常规
    //    安装位置」最成立的场景 —— 正是这两个谓词绝不可互换的那条不变量。
    if let Some(id) = super::identify::script_identity_step(
        full_command,
        short_command,
        short_command,
        short_command,
        is_conventional_install_path,
        |script| AppIdentity {
            label: basename(script).to_string(),
            category: SYSTEM_CATEGORY.to_string(),
        },
    ) {
        return id;
    }

    // 0b. 一次性自动化浏览器实例 —— 身份在命令行，不在路径（与阶梯 0 的脚本身份
    //     完全对称）。必须先于 .app / /Applications 阶梯：headless Chrome 的宿主
    //     可执行文件就住在 /Applications，被归 installed-app 即吃硬豁免、永远漏网
    //     （KNOWN-GAPS Gap 1 的真实案例：空转 7 小时、子进程满核）。
    if super::identify::is_automation_instance(full_command) {
        return AppIdentity {
            label: super::identify::automation_label(exe, short_command),
            category: AUTOMATION_CATEGORY.to_string(),
        };
    }

    // 0c. 非 .app 形态的 dev 运行时 —— Playwright 新默认的 chromium_headless_shell、
    //     node_modules/@esbuild/.../bin/esbuild、~/.cache/selenium 下的 driver 都没有
    //     .app 包装。Windows 侧同判定是无条件阶梯（windows.rs 0c），macOS 曾只在
    //     .app 分支内检查 —— 同一进程两平台置信度分档不同，无端口孤儿在 macOS
    //     整行不可见（评审发现的调用位置漂移）。.app 形态留给阶梯 1 接住，取更
    //     友好的 app 名作标签。
    if !exe.contains(".app/") && super::identify::is_dev_tool_runtime_path(exe) {
        return AppIdentity {
            label: basename(exe).to_string(),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 1. .app bundle —— 抽出 .app 名（exe 来自 ps comm，含空格也完整）。
    //    取法收敛在 identify::app_bundle_name 一处：这里曾与 automation_label 各写
    //    一份逐字相同的 find/rfind，而真机上有嵌套 bundle，两处一旦分叉，同一进程的
    //    两个标签会指向不同的 app 名（评审发现）。
    if let Some(app_name) = super::identify::app_bundle_name(exe) {
        // 开发工具自带 / 下载的 .app 是项目本地的开发 runtime —— electron 把
        // Electron.app 装在 node_modules/electron/dist、Playwright 把 Chromium.app
        // 下载到 ~/Library/Caches/ms-playwright，形态与 /Applications 里的真应用
        // 一模一样。它们不是用户安装的应用，不能享受 installed-app 豁免，否则被杀掉
        // 父进程的孤儿 dev runtime 会因「长得像已安装应用」永远漏网。
        // 用户安装的应用绝不会住在这些目录里，故此信号零误伤（判定见 identify.rs）。
        if super::identify::is_dev_tool_runtime_path(exe) {
            return AppIdentity {
                label: app_name.to_string(),
                category: DEV_SCRIPT_CATEGORY.to_string(),
            };
        }
        let category = if exe.starts_with("/System/") || exe.starts_with("/Library/") {
            SYSTEM_CATEGORY
        } else {
            INSTALLED_APP_CATEGORY
        };
        return AppIdentity {
            label: app_name.to_string(),
            category: category.to_string(),
        };
    }

    // 2a. /Applications/ 下的裸二进制
    if exe.starts_with("/Applications/") {
        return AppIdentity {
            label: basename(exe).to_string(),
            category: INSTALLED_APP_CATEGORY.to_string(),
        };
    }

    // 2b. 系统组件（与 is_standard_install_path 共用 SYSTEM_COMPONENT_PREFIXES）
    //     （曾编号 2c —— 当年 2b 台阶被删后编号未回收，首读会去找不存在的分支）
    if SYSTEM_COMPONENT_PREFIXES.iter().any(|p| exe.starts_with(p)) {
        return AppIdentity {
            label: basename(exe).to_string(),
            category: SYSTEM_CATEGORY.to_string(),
        };
    }

    // 3. 无脚本参数的脚本运行时（node REPL 等）—— 按 exe 走原阶梯
    if super::identify::is_script_runtime(short_command) {
        return AppIdentity {
            label: super::identify::script_runtime_label(full_command, short_command),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 4. /usr/local/, /opt/homebrew/, /opt/local/ → 用户安装的 CLI
    if exe.starts_with("/usr/local/")
        || exe.starts_with("/opt/homebrew/")
        || exe.starts_with("/opt/local/")
    {
        return AppIdentity {
            label: basename(exe).to_string(),
            category: USER_BINARY_CATEGORY.to_string(),
        };
    }

    // 5. Rust/Cargo 产物、`go run` 临时编译产物（/private/var/folders/.../go-build*/exe/main）——
    //    必须先于路径豁免给出 dev-script 身份，否则 /private/var/folders/ 的标准路径前缀会把
    //    孤儿 go run 服务整体豁免（评审发现的真实漏报；该前缀本为 App Translocation 设，
    //    而那些路径含 .app/ 早被阶梯 1 接住）。判定片段集中在 identify::is_dev_build_artifact。
    if super::identify::is_dev_build_artifact(exe) {
        return AppIdentity {
            label: project_binary_label(exe),
            category: DEV_SCRIPT_CATEGORY.to_string(),
        };
    }

    // 6. /Users/... → 用户目录下的自定义二进制。
    //    注意类别是 user-binary 而非 dev-script：「位于用户目录」只说明位置，
    //    不构成 dev 证据 —— dev-script 会把裸孤儿二进制直升 Confirmed 入清扫。
    if exe.starts_with("/Users/") {
        return AppIdentity {
            label: project_binary_label(exe),
            category: USER_BINARY_CATEGORY.to_string(),
        };
    }

    // 7. fallback
    let bin = basename(exe);
    let label = if bin.is_empty() {
        short_command.to_string()
    } else {
        bin.to_string()
    };
    AppIdentity {
        label,
        category: UNKNOWN_CATEGORY.to_string(),
    }
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

/// 系统工具固定绝对路径调用（纵深防御）：不经继承的 $PATH 解析，避免被排在
/// /bin 之前的可写目录里的同名二进制劫持。对一个会代用户做破坏性操作（kill）
/// 的工具，钉死系统二进制位置是零成本加固（评审发现）。未知名回退裸名。
/// pub(crate)：kill 路径（platform.rs）共用同一份映射，避免 "kill"/"ps" 的绝对
/// 路径在两处各写一遍而漂移 —— 一处加固、另一处仍可被劫持（评审发现）。
pub(crate) fn system_bin(program: &str) -> &str {
    match program {
        "lsof" => "/usr/sbin/lsof",
        "ps" => "/bin/ps",
        "launchctl" => "/bin/launchctl",
        other => other,
    }
}

/// 单个采集子进程的墙钟上限。
///
/// `Command::output()` **没有超时**，而 `lsof` 是本项目最容易卡死的调用：任何一个
/// 失联的 NFS / SMB / FUSE 挂载点都会让它阻塞在内核态（`lsof -b` 这个选项存在的
/// 理由就是它）。一次挂死的后果不是「这轮扫描慢」，而是**永久不可用**：
/// `spawn_blocking` 线程再也不返回 ⇒ `ScannerState` 的 Mutex 永不释放 ⇒ 此后每轮
/// 轮询都走 `try_lock` 的 `WouldBlock` 分支返回 `ERR_SCAN_BUSY`，托盘计数冻结、
/// 界面永远显示扫描错误，只能退出重开（评审发现：本项目唯一「一次触发就再也回不来」
/// 的故障）。CLI / Raycast 侧则表现为命令直接挂住。
///
/// 取 5s：本机全部采集子进程合计实测 0.155s，30 倍余量；真卡住时也把损失封在一轮里。
const COLLECT_TIMEOUT: Duration = Duration::from_secs(5);

fn cmd_output(program: &str, args: &[&str]) -> Option<String> {
    cmd_output_within(program, args, COLLECT_TIMEOUT)
}

/// 超时可注入的本体 —— 单测用 200ms 跑真实的挂死子进程，不必让测试套等 5 秒。
fn cmd_output_within(program: &str, args: &[&str], timeout: Duration) -> Option<String> {
    let child = match Command::new(system_bin(program))
        .args(args)
        .env("LANG", "en_US.UTF-8")
        .env("LC_ALL", "en_US.UTF-8")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            // 采集层失败不能静默退化为空输出（与 windows.rs 的留痕对齐）：
            // ps 失败 ⇒ 表格凭空清空、launchctl 失败 ⇒ 托管豁免失效，
            // 没有留痕时用户与开发者都拿不到任何线索（评审发现）。
            log::warn!("{program} {args:?} failed to spawn: {e}; scan may be degraded");
            return None;
        }
    };

    // 收尾放到线程里跑，主线程只等一个带超时的信号。
    //
    // 必须用 `wait_with_output()` 而不是「自己轮询 try_wait」：`ps -A` 在这台机器上
    // 就有近百 KB 输出，管道缓冲区（通常 64 KiB）填满后子进程会阻塞在 write 上永不
    // 退出 —— 不并发抽干管道的话，等来的是一个自己造出来的死锁。
    let pid = child.id();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = match rx.recv_timeout(timeout) {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            log::warn!("{program} {args:?} failed to run: {e}; scan may be degraded");
            return None;
        }
        Err(_) => {
            // 超时：按 PID 直接 SIGKILL。**这里没有 PID 复用风险** —— 子进程尚未被
            // wait 回收（`wait_with_output` 还挂在那个线程里），未回收的进程会一直
            // 占着自己的 PID 槽位，内核不会把它分给别人。杀掉后管道关闭，那个线程
            // 随即结束并顺手把它 reap 掉，不留僵尸。
            unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
            log::error!(
                "{program} {args:?} exceeded {:?} and was killed; scan is degraded \
                 (a stale network mount makes lsof hang in the kernel)",
                timeout
            );
            return None;
        }
    };
    // 留痕但不丢弃 stdout：lsof 在「零结果」与「-p 列表中个别 PID 已消失」时
    // 都返回非零退出码，但 stdout 仍是可用的（部分）结果 —— 按非零整体丢弃
    // 会把其余监听者的 cwd 一并清掉。只在 stderr 有实际内容时记录。
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stderr = stderr.trim();
        if !stderr.is_empty() {
            log::warn!(
                "{program} {args:?} exited with {}: {stderr}; scan may be degraded",
                output.status
            );
        }
    }
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// 一次扫描会话的平台状态 —— macOS 侧无需持有任何东西。
///
/// 与 Windows 的 [`super::windows::PlatformState`] 保持相同的形状（编译期多态，
/// 无 trait）：那边持有 `sysinfo::System` 是因为 CPU 百分比来自两次 refresh 的
/// 增量；这边的 `pcpu` 由 `ps` 每次直接给出，天生无状态，故 `warm_up` 是 no-op。
pub(crate) struct PlatformState;

impl PlatformState {
    pub(crate) fn new() -> Self {
        Self
    }

    /// macOS 的 CPU 读数不依赖采样区间，无需预热。
    pub(crate) fn warm_up(&mut self) {}

    /// macOS 的 `pcpu` 由 `ps` 直接给出，与采样区间无关 —— `scan_once` 据此
    /// 跳过那段等待。它曾是无条件的，而 `CpuSampling::default()` 就是
    /// `Interval(200ms)`：CLI / Raycast 每次扫描都白等 200ms，比本机全部采集
    /// 子进程加起来（lsof + 两次 ps + launchctl + cwd lsof，实测 0.155s）还久，
    /// 且换不到任何东西（评审发现）。
    pub(crate) const NEEDS_CPU_INTERVAL: bool = false;
}

impl PlatformState {
    /// 采集一次。**两个不可替代的数据源失败时整体失败**，绝不静默退化成空结果
    /// （评审发现）：`cmd_output` 返回 `None` 只有两种成因 —— 子进程起不来、或被
    /// 看门狗杀掉，两者都意味着这一轮拿到的不是这台机器的真实状态。此前一律
    /// `unwrap_or_default()`，于是 `ps` 一失败 → 进程表凭空清空 → 所有监听者被
    /// 丢弃 → `scan()` 返回空 Vec 且**没有任何错误信号**，界面落进四态空状态里的
    /// 「一切正常」分支，宣布「没有发现任何监听端口」并把托盘计数清零。
    /// CLAUDE.md 自己写着那句话在任何一台 Mac 上都是假的，而这条路径会稳定地打印它。
    ///
    /// 分界线是「这一轮还能不能代表这台机器」：
    /// - lsof 监听表、ps 进程表 ⇒ 不可替代，失败即整体失败；
    /// - ps comm、launchctl ⇒ 降级但仍有意义（前者退化 exe 路径，后者丢托管豁免、
    ///   方向是多报而非漏报，用户看得见），保持 `unwrap_or_default` + 日志留痕；
    /// - cwd / ESTABLISHED ⇒ 纯增强证据，本就是条件调用。
    pub(crate) fn collect(&mut self) -> Result<Collected, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let listeners = parse_lsof(
            &cmd_output("lsof", &["-iTCP", "-sTCP:LISTEN", "-P", "-n", "-FpcLn"])
                .ok_or("lsof (listening sockets) failed or timed out")?,
        );
        let comm_map =
            parse_comm(&cmd_output("ps", &["-A", "-o", "pid=,comm="]).unwrap_or_default());
        let procs = parse_ps(
            &cmd_output(
                "ps",
                &[
                    "-A",
                    "-o",
                    "pid=,ppid=,state=,tty=,etime=,pcpu=,rss=,user=,command=",
                ],
            )
            .ok_or("ps (process table) failed or timed out")?,
            &comm_map,
            now,
        );
        let launchd_pids = parse_launchctl(&cmd_output("launchctl", &["list"]).unwrap_or_default());

        // 仅查监听者的 cwd（一次 lsof，~15 个 PID）：重复 dev server 检测的证据
        let pid_csv = listeners
            .iter()
            .map(|l| l.pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let cwds = if pid_csv.is_empty() {
            HashMap::new()
        } else {
            parse_cwd(
                &cmd_output("lsof", &["-a", "-p", &pid_csv, "-d", "cwd", "-Fpn"])
                    .unwrap_or_default(),
            )
        };

        // 自动化实例的存活性证据（KNOWN-GAPS Gap 1/A2）：调试端口只 LISTEN、零
        // ESTABLISHED 才是真正的「无人认领」。**只对命令行呈现为自动化实例的 PID**
        // 再查一次 lsof —— 日常这个集合为空 ⇒ 零额外开销；刻意不放宽上面那次
        // `-sTCP:LISTEN` 过滤：那会把全机所有 TCP 连接拉进本项目最贵的一次调用。
        // 这里对全进程表跑一遍 is_automation_instance，随后 scan_from 的孤儿循环
        // 会经 identify_app 的阶梯 0b 再跑一遍同一个谓词 —— 确实是重复计算，但
        // **实测不值得为它重构**（评审提出，已量化）：把这段额外重复 10 遍，release
        // 构建的整轮扫描从 0.16s 只升到 0.17s，即单次约 1ms、占 0.6%。把 AppIdentity
        // 缓存穿进采集层要动的是本项目最热也最密集注释的那段代码，换 1ms 不划算。
        // 真要动它之前，请先重测 —— 进程数或谓词复杂度变了，结论才可能变。
        let automation_csv = procs
            .iter()
            .filter(|(_, m)| super::identify::is_automation_instance(&m.full_command))
            .map(|(pid, _)| pid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let established_local_ports = if automation_csv.is_empty() {
            HashMap::new()
        } else {
            parse_established(
                &cmd_output(
                    "lsof",
                    &[
                        "-a",
                        "-p",
                        &automation_csv,
                        "-iTCP",
                        "-sTCP:ESTABLISHED",
                        "-P",
                        "-n",
                        "-Fpn",
                    ],
                )
                .unwrap_or_default(),
            )
        };

        Ok(Collected {
            listeners,
            procs,
            launchd_pids,
            cwds,
            established_local_ports,
        })
    }
}

/// `lsof … -sTCP:ESTABLISHED -Fpn` 输出：pPID 行后跟 n 行
/// `127.0.0.1:9222->127.0.0.1:54191`。取 **`->` 左侧（本地端）** 的端口 ——
/// 调用方与该 PID 的监听端口取交集，才是「有人连着它的调试端口」。
/// 右侧（对端）端口若被误当本地端口，一个正在抓网页的残留浏览器会因大量
/// 出站连接被误判成「有人在用」而豁免（本函数存在的全部意义就是不出这个错）。
fn parse_established(text: &str) -> HashMap<u32, Vec<u16>> {
    let mut map: HashMap<u32, Vec<u16>> = HashMap::new();
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            // 解析失败必须清空（与 parse_lsof / parse_cwd 同一底线）：
            // 否则后续 n 行会归到上一个 PID 名下，把别人的连接算作它的存活证据
            cur = rest.parse().ok();
        } else if let Some(addr) = line.strip_prefix('n') {
            // split_once 而非 split().next()：后者对无 `->` 的行也返回整串（永不为
            // None），校验只能靠另写一次 contains —— 一个读起来像守卫、实际永不触发的
            // 分支。这里让「必须有对端」直接由解析本身表达：无 `->` 的是监听行，
            // 不是连接，不构成存活证据。
            let Some((local, _peer)) = addr.split_once("->") else {
                continue;
            };
            if let (Some(pid), Some(port)) = (
                cur,
                local.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()),
            ) {
                let ports = map.entry(pid).or_default();
                if !ports.contains(&port) {
                    ports.push(port);
                }
            }
        }
    }
    map
}

/// `lsof -a -p <pids> -d cwd -Fpn` 输出：pPID 行后跟 n<路径> 行。
fn parse_cwd(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    let mut cur: Option<u32> = None;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix('p') {
            // parse 失败必须清空而非保留旧值：否则后续 n 行会归到上一个 PID
            // 名下（cwd 张冠李戴）。实践中 p 行恒为数字，守的是解析器底线。
            cur = rest.parse().ok();
        } else if let Some(path) = line.strip_prefix('n') {
            if let Some(pid) = cur {
                map.insert(pid, path.to_string());
            }
        }
    }
    map
}

/// lsof -F 字段前缀模式：p=PID c=命令 L=用户 n=地址。一个 PID 多端口合并。
fn parse_lsof(text: &str) -> Vec<Listener> {
    let mut by_pid: HashMap<u32, Listener> = HashMap::new();
    let mut current_pid: Option<u32> = None;

    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let tag = line.as_bytes()[0];
        // get(1..) 而非 [1..]：后者是字节切片，若行首是多字节 UTF-8 字符会 panic。
        // lsof -F 每行以 ASCII tag 开头、实践中不会触发，但本文件其余解析器全用
        // 边界安全 API（strip_prefix/split_whitespace），不留这个孤点（评审发现）。
        let Some(rest) = line.get(1..) else {
            continue;
        };
        match tag {
            b'p' => {
                // parse 失败清空而非保留旧值：否则后续 c/L/n 行会归到上一个
                // 进程名下（端口/用户张冠李戴）。实践中 p 行恒为数字，
                // 守的是「解析器不信任输入」底线（与多字节防御同标准）。
                current_pid = rest.parse::<u32>().ok();
                if let Some(pid) = current_pid {
                    by_pid.entry(pid).or_insert(Listener {
                        pid,
                        command: String::new(),
                        user: String::new(),
                        ports: vec![],
                    });
                }
            }
            b'c' => {
                if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                    e.command = rest.to_string();
                }
            }
            b'L' => {
                if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                    e.user = rest.to_string();
                }
            }
            b'n' => {
                let addr = rest.split("->").next().unwrap_or(rest);
                if let Some(port) = addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok()) {
                    if let Some(e) = current_pid.and_then(|pid| by_pid.get_mut(&pid)) {
                        if !e.ports.contains(&port) {
                            e.ports.push(port);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    by_pid.into_values().collect()
}

/// `ps -A -o pid=,comm=`：comm 是最后一列，整行截取 —— 含空格的 exe 路径也完整。
/// 这是 exe_path 的权威来源（修复旧版按空格截断 "/Applications/Visual Studio Code.app" 的 bug）。
fn parse_comm(text: &str) -> HashMap<u32, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let line = line.trim_start();
        let Some((pid_str, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if let Ok(pid) = pid_str.parse::<u32>() {
            map.insert(pid, rest.trim().to_string());
        }
    }
    map
}

/// 解析 [[dd-]hh:]mm:ss 形式的 etime 为秒。**任一段不是合法数字 ⇒ None**(解析失败)。
/// kill 路径据此 fail-closed:解析失败绝不能静默当成「0 秒 / 刚启动」—— 那会把一个
/// 真实创建时间很早的进程算成 now,从而绕过 PID 复用容差(评审发现:与本文件
/// 其余解析器的 fail-closed 风格保持一致)。
pub(crate) fn parse_etime_checked(s: &str) -> Option<u64> {
    let (days, rest) = match s.split_once('-') {
        Some((d, r)) => (d.parse::<u64>().ok()?, r),
        None => (0, s),
    };
    let parts: Vec<u64> = rest
        .split(':')
        .map(|p| p.parse::<u64>().ok())
        .collect::<Option<_>>()?;
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (*h, *m, *s),
        [m, s] => (0, *m, *s),
        [s] => (0, 0, *s),
        _ => return None,
    };
    Some(days * 86_400 + h * 3_600 + m * 60 + sec)
}

/// snapshot 的 elapsed_secs 用:解析失败退化为 0(=「刚启动」=落入 grace 宽限,
/// 偏保守不误扫)。kill 路径请改用 parse_etime_checked 走 fail-closed。
pub(crate) fn parse_etime(s: &str) -> u64 {
    parse_etime_checked(s).unwrap_or(0)
}

/// 解析 ps 输出。数值列在 command 之前；command 是最后一列收尾全行。
///
/// 会话首进程的判定用 state 的 `s` 标志（BSD ps：「s = session leader」）——
/// **不要**用 `ps -o sess=`：macOS 上它对所有进程恒输出 0（内核会话指针对
/// 非特权调用方不可见），曾导致 leader 集合恒空、所有终端进程被误判
/// tty_orphaned（评审捕获的真机级 bug）。
fn parse_ps(text: &str, comm_map: &HashMap<u32, String>, now: u64) -> HashMap<u32, ProcMeta> {
    struct Row {
        pid: u32,
        meta: ProcMeta,
    }
    let mut rows: Vec<Row> = Vec::new();

    for line in text.lines() {
        let line = line.trim_start();
        let mut iter = line.split_whitespace();
        let Some(pid) = iter.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let Some(ppid) = iter.next().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        let state = iter.next().unwrap_or("").to_string();
        let tty = iter.next().unwrap_or("?").to_string();
        let elapsed_secs = parse_etime(iter.next().unwrap_or("0"));
        let cpu_percent: f32 = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0.0);
        let rss_kb: u64 = iter.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        // macOS 账户短名不允许空格，单 token 安全；监听者仍优先用 lsof L 字段，
        // 这一列主要补齐「无端口孤儿」行的 user（此前恒空，评审发现）。
        let user = iter.next().unwrap_or("").to_string();
        let full_command: String = iter.collect::<Vec<_>>().join(" ");

        // exe 权威来源：comm（含空格完整）；退回命令行首 token
        let exe_path = comm_map.get(&pid).cloned().unwrap_or_else(|| {
            full_command
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        });

        rows.push(Row {
            pid,
            meta: ProcMeta {
                ppid,
                exe_path,
                full_command,
                user,
                start_unix: Some(now.saturating_sub(elapsed_secs)),
                elapsed_secs,
                cpu_percent,
                rss_kb,
                tty: Some(tty),
                state: Some(state),
                tty_orphaned: false,
            },
        });
    }

    // 仍有会话首进程（state 含 's'）的 tty 集合 —— 真机验证：每个活跃 ttys
    // 恰有一个带 's' 标志的进程（登录 shell 的 "Ss"/"Ss+"）
    let leader_ttys: HashSet<String> = rows
        .iter()
        .filter(|r| r.meta.state.as_deref().is_some_and(|st| st.contains('s')))
        .filter_map(|r| r.meta.tty.as_deref())
        .filter(|t| t.starts_with("ttys"))
        .map(String::from)
        .collect();

    let mut map = HashMap::new();
    for mut row in rows {
        if let Some(tty) = row.meta.tty.as_deref() {
            // 有真实终端、但该终端已无会话首进程 ⇒ 终端会话已死（iTerm 崩溃等）
            if tty.starts_with("ttys") && !leader_ttys.contains(tty) {
                row.meta.tty_orphaned = true;
            }
        }
        map.insert(row.pid, row.meta);
    }
    map
}

/// `launchctl list` 输出：PID\tStatus\tLabel；首列为 "-" 表示未运行。
/// 能解析出数字 PID 的行即「launchd 当前托管的进程」。
fn parse_launchctl(text: &str) -> HashSet<u32> {
    text.lines()
        .filter_map(|l| l.split_whitespace().next())
        .filter_map(|t| t.parse::<u32>().ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 采集看门狗：挂死的子进程必须被杀掉并如实报失败，而不是把整条扫描拖住。
    ///
    /// 这是本项目唯一「一次触发就再也回不来」的故障：`Command::output()` 没有超时，
    /// 一个卡在失联网络挂载上的 lsof 会让 spawn_blocking 线程永不返回、`ScannerState`
    /// 的锁永不释放，此后每轮轮询都只能返回 `ERR_SCAN_BUSY`（评审发现）。
    ///
    /// 用真实的挂死子进程测，不是 mock —— 要验证的恰恰是「真的能把它杀掉」。
    #[test]
    fn hung_subprocess_is_killed_instead_of_hanging_the_scan() {
        let started = std::time::Instant::now();
        let out = cmd_output_within("sleep", &["30"], Duration::from_millis(200));
        let elapsed = started.elapsed();

        assert!(
            out.is_none(),
            "超时的采集必须如实返回 None，不能假装拿到了空输出"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "看门狗没有生效：等了 {elapsed:?}，说明还在等子进程自己退出"
        );
    }

    /// 超时的子进程必须**彻底消失**：既不能还活着（漏了 SIGKILL —— 每 2 秒一轮的
    /// 轮询会不断堆积挂死进程），也不能变成僵尸（杀了没回收）。回收发生在那个持有
    /// `wait_with_output` 的线程里 —— 管道一关它就返回，顺手把子进程 reap 掉。
    ///
    /// 用一个不会与旁人相撞的独特时长做标记，才能断言「这一个」的下场，而不是去数
    /// 全机所有 sleep。
    #[test]
    fn timed_out_subprocess_is_neither_alive_nor_zombie() {
        const MARKER: &str = "27182818";
        cmd_output_within("sleep", &[MARKER], Duration::from_millis(100));
        // 给那个收尾线程一点时间完成 reap
        std::thread::sleep(Duration::from_millis(400));

        let ps = cmd_output("ps", &["-A", "-o", "stat=,command="]).unwrap_or_default();
        let survivors: Vec<&str> = ps.lines().filter(|l| l.contains(MARKER)).collect();
        assert!(
            survivors.is_empty(),
            "超时的采集子进程既没被杀死也没被回收，残留：{survivors:?}"
        );
    }

    /// 正常命令不受看门狗影响（防止「为了修挂死把正常路径也改坏了」）。
    #[test]
    fn normal_subprocess_still_returns_stdout() {
        let out = cmd_output("ps", &["-A", "-o", "pid="]).expect("ps 应当正常返回");
        assert!(out.lines().count() > 1, "ps 输出异常：{out:?}");
    }

    #[test]
    fn etime_formats() {
        assert_eq!(parse_etime("05"), 5);
        assert_eq!(parse_etime("01:02"), 62);
        assert_eq!(parse_etime("01:02:03"), 3723);
        assert_eq!(parse_etime("2-01:02:03"), 2 * 86400 + 3723);
    }

    #[test]
    fn etime_checked_rejects_garbage() {
        // 合法输入与旧版同值
        assert_eq!(parse_etime_checked("01:02:03"), Some(3723));
        assert_eq!(parse_etime_checked("2-01:02:03"), Some(2 * 86400 + 3723));
        // 任一段非法数字 ⇒ None(kill 路径据此 fail-closed,绝不退化为 0)
        assert_eq!(parse_etime_checked(""), None);
        assert_eq!(parse_etime_checked("oops"), None);
        assert_eq!(parse_etime_checked("01:xx"), None);
        assert_eq!(parse_etime_checked("z-01:02"), None);
        // 旧版仍对垃圾静默退化为 0(parse_ps 的 grace 宽限用,偏保守)
        assert_eq!(parse_etime("oops"), 0);
    }

    #[test]
    fn comm_map_keeps_spaces() {
        let text =
            "  123 /Applications/Visual Studio Code.app/Contents/MacOS/Electron\n  456 /bin/zsh\n";
        let map = parse_comm(text);
        assert_eq!(
            map.get(&123).unwrap(),
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
        );
        assert_eq!(map.get(&456).unwrap(), "/bin/zsh");
    }

    #[test]
    fn lsof_field_mode() {
        let text =
            "p123\ncnode\nLfhf\nn*:5173\nn127.0.0.1:5174\np456\ncpostgres\nLfhf\nn127.0.0.1:5432\n";
        let mut ls = parse_lsof(text);
        ls.sort_by_key(|l| l.pid);
        assert_eq!(ls.len(), 2);
        assert_eq!(ls[0].pid, 123);
        assert_eq!(ls[0].command, "node");
        assert_eq!(ls[0].ports, vec![5173, 5174]);
        assert_eq!(ls[1].ports, vec![5432]);
    }

    /// 回归（评审发现）：行首是多字节 UTF-8 字符时不得 panic ——
    /// 旧实现 `&line[1..]` 按字节切片，切在续字节上直接崩掉整次扫描。
    /// lsof -F 实践中行首恒为 ASCII tag，此处守住的是「解析器不信任输入」底线。
    #[test]
    fn lsof_multibyte_leading_line_is_skipped_not_panicking() {
        let text = "p123\ncnode\n中文噪声行\nn*:5173\n";
        let ls = parse_lsof(text);
        assert_eq!(ls.len(), 1);
        assert_eq!(ls[0].ports, vec![5173]);
    }

    /// 回归（评审发现）：p 行解析失败必须清空 current_pid —— 否则其后的
    /// c/L/n 行会归到上一个进程名下（端口/用户张冠李戴）。
    #[test]
    fn lsof_bad_pid_line_does_not_pollute_previous_process() {
        let text = "p123\ncnode\nn*:5173\npGARBAGE\ncpostgres\nn127.0.0.1:5432\n";
        let ls = parse_lsof(text);
        assert_eq!(ls.len(), 1, "坏 p 行之后的字段必须被丢弃");
        assert_eq!(ls[0].pid, 123);
        assert_eq!(ls[0].command, "node", "不得被后续 c 行覆盖");
        assert_eq!(ls[0].ports, vec![5173], "不得吸入后续 n 行端口");
    }

    #[test]
    fn cwd_parse_and_bad_pid_containment() {
        let text = "p100\nfcwd\nn/Users/x/proj\npBAD\nn/should/be/dropped\np200\nfcwd\nn/srv/app\n";
        let map = parse_cwd(text);
        assert_eq!(map.len(), 2);
        assert_eq!(map.get(&100).unwrap(), "/Users/x/proj");
        assert_eq!(map.get(&200).unwrap(), "/srv/app");
    }

    /// 存活性证据的解析（KNOWN-GAPS Gap 1/A2）：只取 `->` **左侧**的本地端口。
    /// 取错方向的后果是实打实的漏报：一个正在抓网页的残留浏览器有大量出站连接，
    /// 按对端端口统计会让它永远「看起来有人在用」。
    #[test]
    fn established_parses_local_side_only() {
        let text = "\
p397
n127.0.0.1:9222->127.0.0.1:54191
n192.168.1.10:54999->93.184.216.34:443
p400
n[::1]:9333->[::1]:60123
";
        let map = parse_established(text);
        assert_eq!(map.get(&397).unwrap(), &vec![9222, 54999], "取本地端端口");
        assert_eq!(map.get(&400).unwrap(), &vec![9333], "IPv6 形态同样取本地端");
    }

    /// 监听行（无 `->`）不是连接，不得被算成存活证据；坏 p 行必须清空当前 PID，
    /// 否则后续 n 行会把别人的连接算到上一个进程头上（与 parse_lsof 同一底线）。
    #[test]
    fn established_ignores_listen_rows_and_contains_bad_pid() {
        let text = "p100\nn*:9222\nn127.0.0.1:9222->127.0.0.1:5000\npBAD\nn127.0.0.1:8888->127.0.0.1:6000\n";
        let map = parse_established(text);
        assert_eq!(map.get(&100).unwrap(), &vec![9222]);
        assert_eq!(map.len(), 1, "坏 p 行之后的连接必须被丢弃");
    }

    /// 生产真实形态的 ps 行（无 sess 列；state 的 's' 标志 = 会话首进程，
    /// 与真机 `ps -ax -o stat=,tty=` 输出一致）。
    #[test]
    fn ps_parse_realistic_rows_spaced_exe_and_dead_session() {
        let mut comm = HashMap::new();
        comm.insert(
            200u32,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron".to_string(),
        );
        // pid ppid state tty etime pcpu rss user command...
        let text = "\
  100     1 Ss   ??       01:00:00  0.0   1024 _postgres /opt/homebrew/opt/postgresql@16/bin/postgres -D /opt/homebrew/var
  200   150 S    ttys003  00:10:00  1.5  20480 fhf /Applications/Visual Studio Code.app/Contents/MacOS/Electron --type=utility
  300   200 S+   ttys007  00:05:00  0.2   5120 fhf node /Users/x/proj/node_modules/.bin/vite
";
        let procs = parse_ps(text, &comm, 1_000_000);
        let pg = procs.get(&100).unwrap();
        assert_eq!(pg.ppid, 1);
        assert_eq!(pg.elapsed_secs, 3600);
        assert_eq!(pg.start_unix, Some(1_000_000 - 3600));
        // user 列补齐无端口孤儿行（监听者仍优先 lsof L 字段）
        assert_eq!(pg.user, "_postgres");
        assert_eq!(procs.get(&300).unwrap().user, "fhf");
        assert_eq!(
            procs.get(&300).unwrap().full_command,
            "node /Users/x/proj/node_modules/.bin/vite"
        );
        // postgres 在 ?? 上：tty 信号永远中性
        assert!(!pg.tty_orphaned);
        // comm 修复空格路径
        let code = procs.get(&200).unwrap();
        assert_eq!(
            code.exe_path,
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron"
        );
        // ttys003 / ttys007 上没有任何 state 含 's' 的进程 ⇒ 会话已死
        assert!(code.tty_orphaned);
        assert!(procs.get(&300).unwrap().tty_orphaned);
    }

    /// 回归（评审捕获的真机 bug）：健康终端 —— 登录 zsh 是 "Ss" 会话首进程，
    /// 同 tty 的 dev server 绝不能被标 tty_orphaned。
    #[test]
    fn ps_healthy_terminal_never_tty_orphaned() {
        let comm = HashMap::new();
        let text = "\
  500     1 Ss   ttys003  01:00:00  0.0   1024 fhf -zsh
  501   500 S+   ttys003  00:30:00  0.1   2048 fhf node server.js
  600     1 Ss+  ttys000  02:00:00  0.0   1024 fhf -zsh
";
        let procs = parse_ps(text, &comm, 1_000_000);
        assert!(!procs.get(&500).unwrap().tty_orphaned);
        assert!(
            !procs.get(&501).unwrap().tty_orphaned,
            "活终端里的 dev server 不能误报会话死"
        );
        assert!(!procs.get(&600).unwrap().tty_orphaned);
    }

    #[test]
    fn launchctl_pid_extraction() {
        let text = "PID\tStatus\tLabel\n123\t0\tcom.example.agent\n-\t0\tcom.example.idle\n77\t0\thomebrew.mxcl.postgresql@16\n";
        let pids = parse_launchctl(text);
        assert!(pids.contains(&123));
        assert!(pids.contains(&77));
        assert_eq!(pids.len(), 2);
    }

    #[test]
    fn identify_ladder() {
        // .app bundle（含空格路径）
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron --type=utility",
            "Electron",
            "/Applications/Visual Studio Code.app/Contents/MacOS/Electron",
        );
        assert_eq!(label, "Visual Studio Code");
        assert_eq!(cat, "installed-app");

        // node_modules 下的 Electron.app（electron / electron-vite 的 dev runtime）：
        // 形态与 /Applications 的真应用相同，但必须归 dev-script 才不会被 installed-app
        // 豁免吞掉 —— 否则孤儿 Electron（dev 残留）永远检测不到。
        let AppIdentity { label, category: cat } = identify_app(
            "/Users/x/proj/node_modules/.pnpm/electron@33.4.11/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron .",
            "Electron",
            "/Users/x/proj/node_modules/.pnpm/electron@33.4.11/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron",
        );
        assert_eq!(label, "Electron");
        assert_eq!(cat, "dev-script");

        // 系统组件
        let AppIdentity {
            label,
            category: cat,
        } = identify_app("/usr/sbin/cupsd -l", "cupsd", "/usr/sbin/cupsd");
        assert_eq!(label, "cupsd");
        assert_eq!(cat, "system");

        // 脚本运行时 + 项目提取
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "node /Users/x/proj/node_modules/vite/bin/vite.js",
            "node",
            "/usr/local/bin/node",
        );
        assert_eq!(label, "proj · vite.js");
        assert_eq!(cat, "dev-script");

        // Homebrew CLI
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "/opt/homebrew/bin/redis-server *:6379",
            "redis-server",
            "/opt/homebrew/bin/redis-server",
        );
        assert_eq!(label, "redis-server");
        assert_eq!(cat, "user-binary");

        // Cargo 产物
        let AppIdentity { category: cat, .. } = identify_app(
            "/Users/x/rust/mytool/target/debug/mytool",
            "mytool",
            "/Users/x/rust/mytool/target/debug/mytool",
        );
        assert_eq!(cat, "dev-script");

        // 回归（评审发现的真实漏报）：go run 临时编译产物在 /private/var/folders
        // 下，必须拿到 dev-script 身份 —— 否则被标准路径前缀整体豁免，
        // 孤儿 go run 服务永远不可见（CLAUDE.md 明示 cargo run 同类是产品目标）
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "/private/var/folders/dx/T/go-build123/b001/exe/server --port 8080",
            "server",
            "/private/var/folders/dx/T/go-build123/b001/exe/server",
        );
        assert_eq!(cat, "dev-script");
        assert_eq!(label, "server");

        // `-m 模块` 调用：身份是模块，不因解释器在 brew/系统路径而改类。
        // 必须用完整真实路径：brew 的 Python 住在 Cellar 的 Python.app bundle
        // 里，曾被 .app 阶梯先一步归 installed-app 豁免（真实漏报案例：
        // 孤儿 python -m http.server 占着 8000 端口）。
        let AppIdentity { label, category: cat } = identify_app(
            "/opt/homebrew/Cellar/python@3.14/3.14.5/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python -m http.server 8000",
            "Python",
            "/opt/homebrew/Cellar/python@3.14/3.14.5/Frameworks/Python.framework/Versions/3.14/Resources/Python.app/Contents/MacOS/Python",
        );
        assert_eq!(label, "http.server · Python");
        assert_eq!(cat, "dev-script");

        // 同形态跑用户脚本：身份是脚本，.app 包装不豁免
        let AppIdentity { label, category: cat } = identify_app(
            "/Library/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python /Users/x/bot/main.py",
            "Python",
            "/Library/Frameworks/Python.framework/Versions/3.12/Resources/Python.app/Contents/MacOS/Python",
        );
        assert_eq!(label, "main.py · Python");
        assert_eq!(cat, "dev-script");

        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "/usr/bin/python3 -m http.server 9000",
            "python3",
            "/usr/bin/python3",
        );
        assert_eq!(label, "http.server · python3");
        assert_eq!(cat, "dev-script");

        // 脚本落在 TMPDIR（安装器 / 测试框架的常见形态）必须是 dev-script。
        // 阶梯 0 曾把**豁免**谓词 is_standard_install_path 当**事实**谓词用，而前者
        // 刻意收了 /private/var/folders/ —— 那条 carve-out 是给 App Translocation 的
        // .app 让路的，.app 要到阶梯 1 才处理。误用的后果：临时目录里的脚本被判成
        // 「系统自带脚本任务」→ 类别 system → 吃下 InstalledApp 硬豁免、永不标记，
        // 无端口时整行都不出现（评审发现）。
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            "/usr/bin/python3 /private/var/folders/dx/T/tmpXYZ/serve.py",
            "python3",
            "/usr/bin/python3",
        );
        assert_eq!(cat, "dev-script", "临时目录里的脚本不是系统自带脚本任务");
        assert_eq!(label, "serve.py · python3");

        // 对照：真正住在系统路径下的脚本仍归 system —— 上面的修正不得把这条打死
        assert_eq!(
            identify_app(
                "/usr/bin/python3 /System/Library/Foo/bar.py",
                "python3",
                "/usr/bin/python3",
            )
            .category,
            "system",
        );

        // KNOWN-GAPS Gap 1：headless 自动化实例的身份在命令行 —— 必须先于
        // .app / /Applications 阶梯判定，否则归 installed-app 即吃硬豁免。
        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(
            &format!(
                "{CHROME} --headless=new --user-data-dir=/private/tmp/sess/prof \
                 --remote-debugging-port=9339 about:blank"
            ),
            "Google Chrome",
            CHROME,
        );
        assert_eq!(label, "Google Chrome · headless");
        assert_eq!(cat, super::super::AUTOMATION_CATEGORY);

        // 对照：同一个 exe，无自动化开关 ⇒ 仍是用户日常那个 Chrome
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(CHROME, "Google Chrome", CHROME);
        assert_eq!(label, "Google Chrome");
        assert_eq!(cat, "installed-app");

        // Playwright 下载到 Caches 的 Chromium.app：形态同真应用，但归 dev-script
        //（与 node_modules 下的 Electron.app 同一条不变量）
        let pw = "/Users/x/Library/Caches/ms-playwright/chromium-1148/chrome-mac/Chromium.app/Contents/MacOS/Chromium";
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(pw, "Chromium", pw);
        assert_eq!(label, "Chromium");
        assert_eq!(cat, "dev-script");
    }

    /// 阶梯 0c：非 .app 形态的 dev 运行时也必须归 dev-script —— 此前该判定只在
    /// .app 分支内做，Windows 是无条件阶梯（评审发现的调用位置漂移：同一进程
    /// macOS 降档 user-binary，置信度分层与无端口孤儿门全部受损）。
    #[test]
    fn dev_tool_runtime_without_app_bundle_is_dev_script() {
        // Playwright 新默认下载的 headless shell（无 .app 包装）
        let hs = "/Users/x/Library/Caches/ms-playwright/chromium_headless_shell-1155/chrome-mac/headless_shell";
        let AppIdentity {
            label,
            category: cat,
        } = identify_app(hs, "headless_shell", hs);
        assert_eq!(label, "headless_shell");
        assert_eq!(cat, "dev-script");

        // Selenium Manager 下载的 chromedriver
        let cd = "/Users/x/.cache/selenium/chromedriver/mac-arm64/chromedriver --port=9515";
        let exe = "/Users/x/.cache/selenium/chromedriver/mac-arm64/chromedriver";
        assert_eq!(identify_app(cd, "chromedriver", exe).category, "dev-script");

        // node_modules 下的平台二进制（esbuild）
        let es = "/Users/x/proj/node_modules/@esbuild/darwin-arm64/bin/esbuild --serve";
        let exe = "/Users/x/proj/node_modules/@esbuild/darwin-arm64/bin/esbuild";
        assert_eq!(identify_app(es, "esbuild", exe).category, "dev-script");

        // 对照：用户目录下的普通二进制不受影响，仍是 user-binary
        let ub = "/Users/x/bin/mytool";
        assert_eq!(identify_app(ub, "mytool", ub).category, "user-binary");
    }

    /// `.app/` 兜底刻意**不**服从 `identify_app` 的身份判定 —— 这与
    /// 「dev 工具运行时归 dev-script 而非 installed-app」不矛盾，两者管的是
    /// 不同的事：那条不变量防的是**豁免**（别让 Electron 吃 installed-app
    /// 硬豁免），这里管的是**链终点**（helper 的父就是它，父健在就不该把每个
    /// helper 摊成独立一行）。
    ///
    /// 评审建议过在此按 category 早退（dev-script / automation-instance 不停），
    /// 会直接击穿 `busy_helper_under_live_parent_is_not_listed_separately`：
    /// helper 的链穿过健在的主进程一路走到 ppid=1，判成 chain_orphan，Gap 1
    /// 主案的每个 GPU/renderer 子进程都会变成独立可疑行。真正的孤儿 dev 运行时
    /// 由它**自己那一行**（ppid=1 ⇒ direct_orphan）呈现，不依赖链回溯。
    #[test]
    fn chain_stopper_stops_at_dev_runtimes_on_purpose() {
        const CHROME: &str = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

        // 真实用户可见 App：照常终止链
        assert!(is_chain_stopper(CHROME, "installed-app"));
        // 系统自带 Terminal.app 类别是 system，靠 .app/ 兜住
        assert!(is_chain_stopper(
            "/System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal",
            "system"
        ));

        // node_modules 里的 Electron：身份是 dev-script，但**照样**是链终点
        let electron =
            "/Users/x/proj/node_modules/electron/dist/Electron.app/Contents/MacOS/Electron";
        assert_eq!(
            identify_app(electron, "Electron", electron).category,
            "dev-script"
        );
        assert!(
            is_chain_stopper(electron, "dev-script"),
            "父健在的 Electron 主进程必须终止链，否则其 renderer 会被误报为孤儿"
        );

        // Playwright 的 Chromium.app、headless 自动化实例：同理
        let pw = "/Users/x/Library/Caches/ms-playwright/chromium-1148/chrome-mac/Chromium.app/Contents/MacOS/Chromium";
        assert!(is_chain_stopper(pw, "dev-script"));
        assert!(is_chain_stopper(CHROME, super::super::AUTOMATION_CATEGORY));

        // 但非 .app 形态的 dev 进程不是链终点（原有行为不变）
        assert!(!is_chain_stopper("/opt/homebrew/bin/node", "dev-script"));
    }

    /// 豁免谓词与事实谓词必须在 App Translocation 目录上**给出相反答案** ——
    /// 这正是评审捕获的坑：拿豁免谓词陈述事实，`go run` 的临时编译产物
    ///（真住在 /private/var/folders/，identify_app 归 dev-script）会被断言成
    /// 「装在标准位置」，从而丢掉 NonstandardPath —— 而那恰是这条理由最成立的场景。
    #[test]
    fn conventional_path_excludes_the_translocation_carveout() {
        const GO_RUN: &str = "/private/var/folders/dx/T/go-build123/b001/exe/server";
        assert!(
            is_standard_install_path(GO_RUN),
            "豁免侧必须继续放行（App Translocation 的既有语义不动）"
        );
        assert!(
            !is_conventional_install_path(GO_RUN),
            "事实侧必须说实话：临时目录不是常规安装位置"
        );

        // 两者一致的部分：真正的安装位置
        for p in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/usr/bin/python3",
            "/System/Library/CoreServices/ControlCenter.app/Contents/MacOS/ControlCenter",
        ] {
            assert!(is_standard_install_path(p), "{p}");
            assert!(is_conventional_install_path(p), "{p}");
        }
        // 两者一致的部分：确实非标准（brew、用户目录、读不到 exe）
        for p in ["/opt/homebrew/bin/node", "/Users/x/.vite-plus/node", ""] {
            assert!(!is_standard_install_path(p), "{p}");
            assert!(!is_conventional_install_path(p), "{p}");
        }
    }

    #[test]
    fn brew_service_detection() {
        assert!(is_brew_service_path(
            "/opt/homebrew/opt/postgresql@16/bin/postgres"
        ));
        assert!(is_brew_service_path(
            "/usr/local/Cellar/redis/7.2/bin/redis-server"
        ));
        assert!(!is_brew_service_path("/opt/homebrew/bin/node"));
        assert!(!is_brew_service_path("/Users/x/bin/server"));
    }

    #[test]
    fn shell_detection_handles_login_dash() {
        assert!(is_shell("/bin/zsh"));
        assert!(is_shell("-zsh"));
        assert!(is_shell("/opt/homebrew/bin/fish"));
        assert!(!is_shell("/usr/local/bin/node"));
    }
}
