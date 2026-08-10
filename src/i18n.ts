// 轻量类型安全 i18n —— 不引库：
// - key 类型从 zh 字典推导，en 用 Record<Key, string> 约束 → 缺/多 key 都是 tsc 编译错误
// - reason.* / reasonTip.* / story.* / verdict.* 四个键族与 Rust 端 ReasonCode/
//   Confidence 的 serde 名一一对应，CI 的 scripts/check-reason-parity.mjs 做机械校验
// - story.* 是产品层文案：把主判定码翻译成一句人话结论（行内展示）；
//   reason.*（短标签）+ reasonTip.*（完整解释）用于展开的详情面板
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

const zh = {
  // ---- toolbar ----
  "search.placeholder": "搜索 端口 / PID / App / 启动方",
  "filter.all": "全部",
  "filter.suspect": "可疑",
  "filter.whitelist": "收藏",
  "sweep.button": "一键清理",
  "sweep.sweeping": "清理中…",
  // 层级用词与行内 verdict.* 标签严格一致（确认僵尸/疑似僵尸/存疑）——
  // 用户在弹窗里读到的层级名必须能在列表里找到（评审发现：曾写「很可能」）
  "sweep.title": "批量终止「确认僵尸」与「疑似僵尸」级别（「存疑」不会被清扫）",

  // ---- sections ----
  // 注意三层词汇不撞车：标签页「可疑」› 分区「可疑进程」› 行内判定「确认僵尸/疑似僵尸/存疑」
  "section.suspects": "可疑进程",
  "section.suspects.sub": "来源已退出、同项目重复启动，或已脱离父进程的残留进程",
  "section.healthy": "正常监听",
  "section.starred": "已收藏",
  allclear: "未发现僵尸进程，一切正常",

  // ---- footer ----
  "footer.status": "{procs} 个进程 · {ports} 个端口 · 每 2 秒自动扫描",

  // ---- errors ----
  "error.clickToClose": "· 点击关闭",
  "error.killFailed": "终止失败: {err}",
  "error.openBrowser": "打开浏览器失败: {err}",
  "error.batchFailed": "{failed}/{total} 个进程终止失败：",
  "error.pidReused": "进程身份已变化（PID 被复用），已取消终止，请重新扫描",
  "error.processGone": "进程已不存在（可能刚刚退出）",
  "error.accessDenied": "无权终止该进程（可能是受保护的系统进程）",
  "error.identityUnknown": "缺少进程身份信息，已取消终止，请刷新后重试",
  "error.whitelistFailed": "收藏保存失败（本次更改未持久化）: {err}",
  "error.scanTimeout": "扫描超时（后端无响应），正在自动重试…",
  "error.scanBusy": "上一轮扫描尚未结束，已跳过本轮（扫描可能卡住）",
  "error.actionTimeout": "操作超时（后端无响应），请重试",

  // ---- row ----
  "port.tip": "在浏览器打开 http://localhost:{port}",
  "row.expand.tip": "查看详情",
  "row.noPort": "无端口",
  "row.noPort.tip": "该进程不监听任何端口，因脱离父进程成为孤儿而被检出",
  "row.busySubtree": "子进程 {cpu}% CPU",
  "row.busySubtree.tip":
    "这一行自身几乎不占 CPU，但它的子进程正在持续占用 —— 常见于无头浏览器（渲染 / GPU 子进程）",

  // ---- uptime（粗粒度，精确值在详情）----
  "uptime.now": "刚刚",
  "uptime.min": "{n} 分钟",
  "uptime.hour": "{n} 小时",
  "uptime.day": "{n} 天",

  // ---- verdict（结论前缀，按置信层级）----
  "verdict.confirmed": "确认僵尸",
  "verdict.likely": "疑似僵尸",
  "verdict.possible": "存疑",

  // ---- story（主判定码 → 一句人话；与 ReasonCode 蛇形名对应）----
  "story.defunct": "进程已死，等待回收",
  "story.ppid1_orphan": "启动它的程序已退出",
  "story.orphaned_chain": "启动它的终端已关闭",
  "story.parent_exited": "父进程已退出",
  "story.pid_slot_reused": "原父进程已死",
  "story.orphaned_session": "终端会话已死",
  "story.just_reparented": "刚启动，观察中",
  "story.duplicate_dev_server": "与 PID {pid} 重复启动",
  "story.automation_instance": "自动化测试留下的无头浏览器",
  "story.nonstandard_path": "非标准路径",
  "story.dev_server_keyword": "疑似开发服务器",
  "story.launchedBy": "由 {app} 启动",
  "story.launchedBySystem": "系统启动",
  "story.managedBySystem": "系统托管",
  "story.starred": "已收藏",

  // ---- desc（类别 → 小白能懂的一句话，知识库未命中时的兜底）----
  "desc.installed-app": "已安装应用的服务",
  "desc.system": "系统组件",
  "desc.dev-script": "开发脚本 / 本地服务器",
  "desc.automation-instance": "自动化脚本启动的无头浏览器",
  "desc.user-binary": "命令行工具",
  "desc.unknown": "未知程序",

  // ---- actions ----
  "kill.btn": "终止",
  "kill.btn.tip": "SIGTERM 优雅终止",
  "kill.force.btn": "强杀",
  "kill.force.tip": "SIGKILL 强制终止（不可被忽略）",
  "kill.terminate.btn": "终止",
  "kill.terminate.tip": "TerminateProcess 立即终止（Windows 无优雅终止）",
  "star.add.tip": "收藏（不再被标记为僵尸）",
  "star.remove.tip": "取消收藏",

  // ---- detail panel ----
  "detail.command": "完整命令",
  "detail.path": "可执行文件",
  "detail.ports": "端口",
  "detail.pid": "PID",
  "detail.parent": "父进程",
  "detail.parent.launchdNote": "PID 1 = launchd：原启动者已退出，本进程已被系统收养",
  "detail.state": "进程状态（ps state 标志）",
  // ps 主状态字母的人话。原码照旧并列显示 —— 它是给会看 ps 的用户的证据
  "state.R": "运行中",
  "state.S": "休眠",
  "state.I": "闲置",
  "state.T": "已暂停",
  "state.U": "不可中断等待",
  "state.Z": "已成僵尸",
  // 挂起态的后果必须说清：这正是「终止了没反应」的经典成因
  "detail.state.stopped.tip":
    "进程被挂起（Ctrl-Z，或后台作业读写终端）。已捕获的 SIGTERM 在它恢复运行前不会被处理 —— 终止时 Portreaper 会随后唤醒它，让它自己收尾。",
  "detail.chain": "启动链",
  "detail.chain.empty": "无法回溯",
  "detail.category": "类别",
  "detail.resources": "资源",
  "detail.resources.value": "CPU {cpu}% · 内存 {mem} MB · 已运行 {uptime}",
  // 子树合计：CPU 烧在子进程里时才追加（无头浏览器的 gpu-process 是典型）
  "detail.resources.tree": "· 含子进程共 {cpu}%",
  "detail.resources.tree.tip": "该进程与它全部子进程的 CPU 合计",
  "detail.evidence": "判定依据",
  "detail.whyNot": "为什么不是僵尸",

  // ---- categories ----
  "cat.installed-app": "应用程序",
  "cat.system": "系统进程",
  "cat.dev-script": "开发脚本",
  "cat.automation-instance": "自动化实例",
  "cat.user-binary": "命令行工具",
  "cat.unknown": "未知",

  // ---- reason codes（与 Rust ReasonCode serde 名对应）----
  "reason.defunct": "进程已死 (defunct)",
  "reason.ppid1_orphan": "孤儿 (PPID=1)",
  "reason.parent_exited": "父进程已退出",
  "reason.pid_slot_reused": "父 PID 已被复用",
  "reason.orphaned_chain": "孤儿进程链",
  "reason.orphaned_session": "终端会话已死",
  "reason.nonstandard_path": "非标准安装路径",
  "reason.dev_server_keyword": "dev-server 关键字",
  "reason.duplicate_dev_server": "同项目重复实例",
  "reason.automation_instance": "无头自动化实例",
  "reason.debugger_attached": "正被调试器驱动",
  "reason.launchd_managed": "launchd 托管",
  "reason.brew_service_path": "Homebrew 服务",
  "reason.installed_app": "正规安装位置",
  "reason.pm2_managed": "pm2 托管",
  "reason.just_reparented": "刚被收养 (<10s)",

  "reasonTip.defunct": "ps state 含 Z：进程已退出但未被父进程回收，是教科书意义上的僵尸。",
  "reasonTip.ppid1_orphan":
    "父进程已退出，本进程被 launchd（PID 1）收养 —— 原启动它的终端 / IDE 已经不在了。",
  "reasonTip.parent_exited": "记录的父进程已不存在于进程表中 —— 启动它的程序已退出。",
  "reasonTip.pid_slot_reused":
    "父 PID 指向的进程创建时间晚于本进程 —— 槽位已被无关新进程复用，真实父进程已死。",
  "reasonTip.orphaned_chain":
    "父进程链一路走到系统根，途中没有任何存活的用户可见 App —— 启动它的 shell 已死（如关掉的终端窗口）。",
  "reasonTip.orphaned_session":
    "进程持有真实终端 (ttys)，但该终端已没有会话首进程 —— 终端应用可能已崩溃或被强杀。",
  "reasonTip.nonstandard_path": "可执行文件不在系统 / 应用程序等标准安装位置。",
  "reasonTip.dev_server_keyword": "命令行命中常见 dev-server 关键字（vite / node / uvicorn …）。",
  "reasonTip.duplicate_dev_server":
    "同一项目已有另一个相同的开发服务器实例在监听（端口被顺延或分散在多个终端 / IDE）—— 通常是忘了已经启动过。确认在用哪个后终止另一个；两个都需要时可收藏豁免。",
  // 措辞不得预设「它开着调试端口」：无端口的子进程（渲染 / GPU 进程）孤儿化后
  // 同样带这条理由，那类行根本没有端口可言。
  "reasonTip.automation_instance":
    "命令行是一次性自动化会话的形态（--headless 加调试端口 / 临时用户目录）—— Playwright、Puppeteer、爬虫脚本等启动的无头浏览器。它已脱离启动它的程序，也没有任何客户端在驱动它（调试端口上一旦有客户端连着，就会被判为清白），只剩这个实例及其子进程在空转。",
  "reasonTip.debugger_attached":
    "它的调试端口上有客户端正连着 —— 有程序此刻正在驱动这个浏览器实例，不是残留。终止它会打断正在跑的会话，因此绝不标记。",
  "reasonTip.launchd_managed":
    "launchctl 认领的任务（LaunchAgent / brew services）—— 由 launchd 有意托管，不是僵尸。",
  "reasonTip.brew_service_path":
    "可执行文件位于 Homebrew 服务路径（brew services 启动的 postgres / redis 等），不是僵尸。",
  "reasonTip.installed_app":
    "位于标准安装位置（/Applications、系统目录、Program Files 等），不是僵尸。",
  "reasonTip.pm2_managed": "由 pm2 守护进程托管 —— 用户有意让它常驻，不是僵尸。",
  "reasonTip.just_reparented":
    "启动（或刚被收养）不足 10 秒，可能正处于重启过渡态 —— 暂列存疑，不会被清扫。",

  // ---- empty states ----
  "empty.none": "没有发现任何监听端口",
  "empty.noMatch": "没有匹配项",
  "empty.scanning": "正在扫描…",
  "empty.scanFailed": "这一轮扫描没成功。",
  "empty.retry": "重试",
  "empty.noStarred": "还没有收藏。收藏（★）过的进程永远不会被判为僵尸，也不会进入一键清理。",

  // ---- 终止后的存活确认 ----
  // 「信号送到了」不等于「进程死了」：捕获了 SIGTERM 却不退出的进程会让这条出现
  "kill.survivor": "PID {pid} {label} 收到了终止信号，但还没有退出。",
  "kill.survivor.force": "强制终止",
  "kill.survivor.dismiss": "知道了",

  // ---- batch modal ----
  "batch.title": "清扫 {n} 个疑似僵尸进程",
  "batch.signal": "信号",
  "batch.signal.macos": "SIGTERM (-15) 优雅终止",
  "batch.signal.windows": "TerminateProcess（强制终止）",
  "batch.procs": "进程",
  "batch.more": "… 还有 {n} 个",
  "batch.scrollHint": "共 {n} 个，可滚动查看全部",
  "batch.scope.note": "仅清扫「确认僵尸」与「疑似僵尸」级别；「存疑」需逐个手动处理",
  "batch.cancel": "取消",
  "batch.confirm": "全部终止",

  // ---- confirm modal ----
  "confirm.title.kill": "终止进程",
  "confirm.title.force": "强制杀死进程",
  "confirm.app": "应用",
  "confirm.cmd": "命令",
  "confirm.pid": "PID",
  "confirm.ports": "端口",
  "confirm.portsRelease": "· 杀掉进程会同时释放这 {n} 个端口",
  "confirm.signal": "信号",
  "confirm.signal.term": "SIGTERM (-15) 优雅终止",
  "confirm.signal.kill": "SIGKILL (-9) 不可被忽略",
  "confirm.signal.win": "TerminateProcess（立即强制终止）",
  "confirm.cancel": "取消",
  "confirm.kill": "终止",
  "confirm.force": "强制杀死",
} as const;

export type I18nKey = keyof typeof zh;

// ---- 动态键族的收敛出口（评审发现：组件里散布 7 处 `as I18nKey` 断言，每处
// 都是绕过类型检查的洞）----
// 键族拼接只允许发生在下面这组窄函数里：组件侧零断言，审计面收敛到本文件一处。
// 运行时安全网是 translate() 的既有兜底链（zh → en → 键名，见 translate 注释），
// 正常构建下 CI 的 check-reason-parity.mjs 保证 reason./reasonTip./story./verdict.
// 四族键与 Rust ReasonCode/Confidence 一一对应，不会走到兜底。

/** 唯一的动态键断言点 —— 未知码由 translate() 兜底渲染为键名（可辨认、可追查）。 */
function dynamicKey(candidate: string): I18nKey {
  return candidate as I18nKey;
}

/** ReasonCode → 详情短标签键（reason.*）。 */
export function reasonKey(code: string): I18nKey {
  return dynamicKey(`reason.${code}`);
}

/** ReasonCode → 详情完整解释键（reasonTip.*）。 */
export function reasonTipKey(code: string): I18nKey {
  return dynamicKey(`reasonTip.${code}`);
}

/** 主判定码 → 行内一句话结论键（story.*，仅正向码有）。 */
export function storyKey(code: string): I18nKey {
  return dynamicKey(`story.${code}`);
}

/** 置信层级 → 行内前缀键（verdict.*）。 */
export function verdictKey(confidence: string): I18nKey {
  return dynamicKey(`verdict.${confidence}`);
}

/** 类别 → cat.* 键；未知类别落 cat.unknown。字典本身就是合法类别清单 ——
 *  取代详情面板里手工维护的类别数组（评审发现：数组与字典键重复，会漂移）。 */
export function categoryKey(category: string): I18nKey {
  const k = `cat.${category}`;
  return k in zh ? (k as I18nKey) : "cat.unknown";
}

/**
 * ps state 的**首字母** → 一句人话（state.* 键），认不出返回 null。
 *
 * 只映射首字母、且刻意**不做穷举表**：state 列是「主状态字母 + 若干附加标志」
 * （`Ss+` / `TN` / `Z`），附加标志是平台细节，抄全一份就是又一处要跟着 ps 同步
 * 的手抄清单。认不出时调用方原样显示原码 —— 那本来就是给会看 ps 的用户的证据。
 *
 * `T` 是这里最要紧的一个：被挂起的进程收不到已捕获的 SIGTERM，而在此之前
 * 用户能看到的只有一个孤零零的字母 T 和一句解释不了任何事的 tooltip。
 */
export function stateKey(state: string): I18nKey | null {
  const k = `state.${state.charAt(0)}`;
  return k in zh ? (k as I18nKey) : null;
}

const en: Record<I18nKey, string> = {
  "search.placeholder": "Search port / PID / app / launcher",
  "filter.all": "All",
  "filter.suspect": "Zombies",
  "filter.whitelist": "Starred",
  "sweep.button": "Clean up",
  "sweep.sweeping": "Cleaning…",
  "sweep.title": "Batch-terminate 'Zombie' and 'Likely zombie' rows ('Possible' is never swept)",

  "section.suspects": "Suspects",
  "section.suspects.sub": "Launcher exited, a duplicate instance, or detached from its parent",
  "section.healthy": "Healthy listeners",
  "section.starred": "Starred",
  allclear: "No zombies. All clear",

  "footer.status": "{procs} processes · {ports} ports · rescans every 2s",

  "error.clickToClose": "· click to dismiss",
  "error.killFailed": "Kill failed: {err}",
  "error.openBrowser": "Failed to open browser: {err}",
  "error.batchFailed": "{failed}/{total} processes failed to terminate:",
  "error.pidReused": "Process identity changed (PID was reused) — kill aborted, please rescan",
  "error.processGone": "Process no longer exists (it may have just exited)",
  "error.accessDenied": "Not permitted to terminate this process (it may be protected)",
  "error.identityUnknown": "Missing process identity token — kill aborted, refresh and retry",
  "error.whitelistFailed": "Whitelist update failed (change not persisted): {err}",
  "error.scanTimeout": "Scan timed out (no response from backend); retrying…",
  "error.scanBusy": "Previous scan still running; skipped this round",
  "error.actionTimeout": "Action timed out (no response from backend), please retry",

  "port.tip": "Open http://localhost:{port} in browser",
  "row.expand.tip": "Show details",
  "row.noPort": "no port",
  "row.noPort.tip": "Listens on no port — surfaced because it was orphaned from its parent",
  "row.busySubtree": "{cpu}% CPU in children",
  "row.busySubtree.tip":
    "This row itself is nearly idle, but its child processes are burning CPU — typical of a headless browser (renderer / GPU child processes)",

  "uptime.now": "now",
  "uptime.min": "{n} min",
  "uptime.hour": "{n} hr",
  "uptime.day": "{n} d",

  "verdict.confirmed": "Zombie",
  "verdict.likely": "Likely zombie",
  "verdict.possible": "Possible",

  "story.defunct": "dead, never reaped",
  "story.ppid1_orphan": "its launcher has exited",
  "story.orphaned_chain": "its terminal is gone",
  "story.parent_exited": "parent process exited",
  "story.pid_slot_reused": "original parent is dead",
  "story.orphaned_session": "dead terminal session",
  "story.just_reparented": "just started, watching",
  "story.duplicate_dev_server": "duplicate of PID {pid}",
  "story.automation_instance": "headless browser left by automation",
  "story.nonstandard_path": "non-standard path",
  "story.dev_server_keyword": "looks like a dev server",
  "story.launchedBy": "launched by {app}",
  "story.launchedBySystem": "started by the system",
  "story.managedBySystem": "managed by the system",
  "story.starred": "starred",

  "desc.installed-app": "Service of an installed app",
  "desc.system": "System component",
  "desc.dev-script": "Dev script / local server",
  "desc.automation-instance": "Headless browser started by an automation script",
  "desc.user-binary": "Command-line tool",
  "desc.unknown": "Unknown program",

  "kill.btn": "Kill",
  "kill.btn.tip": "SIGTERM, graceful",
  "kill.force.btn": "Force",
  "kill.force.tip": "SIGKILL, cannot be ignored",
  "kill.terminate.btn": "Terminate",
  "kill.terminate.tip":
    "TerminateProcess — immediate (Windows has no graceful kill for detached processes)",
  "star.add.tip": "Star (never flag as zombie)",
  "star.remove.tip": "Remove star",

  "detail.command": "Command",
  "detail.path": "Executable",
  "detail.ports": "Ports",
  "detail.pid": "PID",
  "detail.parent": "Parent",
  "detail.parent.launchdNote":
    "PID 1 = launchd: the original launcher exited and this process was adopted",
  "detail.state": "Process state (ps state flags)",
  "state.R": "running",
  "state.S": "sleeping",
  "state.I": "idle",
  "state.T": "stopped",
  "state.U": "uninterruptible wait",
  "state.Z": "defunct",
  "detail.state.stopped.tip":
    "The process is suspended (Ctrl-Z, or a background job touching the terminal). A caught SIGTERM stays pending until it resumes — Portreaper wakes it up right after terminating so it can shut itself down.",
  "detail.chain": "Launch chain",
  "detail.chain.empty": "untraceable",
  "detail.category": "Category",
  "detail.resources": "Resources",
  "detail.resources.value": "CPU {cpu}% · {mem} MB RAM · up {uptime}",
  "detail.resources.tree": "· {cpu}% incl. children",
  "detail.resources.tree.tip": "CPU of this process plus all of its child processes",
  "detail.evidence": "Why flagged",
  "detail.whyNot": "Why not a zombie",

  "cat.installed-app": "Application",
  "cat.system": "System process",
  "cat.dev-script": "Dev script",
  "cat.automation-instance": "Automation instance",
  "cat.user-binary": "CLI tool",
  "cat.unknown": "Unknown",

  "reason.defunct": "defunct",
  "reason.ppid1_orphan": "orphan (PPID=1)",
  "reason.parent_exited": "parent exited",
  "reason.pid_slot_reused": "parent PID reused",
  "reason.orphaned_chain": "orphaned chain",
  "reason.orphaned_session": "dead terminal session",
  "reason.nonstandard_path": "non-standard path",
  "reason.dev_server_keyword": "dev-server keyword",
  "reason.duplicate_dev_server": "duplicate instance",
  "reason.automation_instance": "headless automation instance",
  "reason.debugger_attached": "debugger attached",
  "reason.launchd_managed": "launchd-managed",
  "reason.brew_service_path": "Homebrew service",
  "reason.installed_app": "standard install location",
  "reason.pm2_managed": "pm2-managed",
  "reason.just_reparented": "just reparented (<10s)",

  "reasonTip.defunct":
    "ps state contains Z: the process has exited but was never reaped — a textbook zombie.",
  "reasonTip.ppid1_orphan":
    "Its parent exited and it was adopted by launchd (PID 1) — the terminal / IDE that started it is gone.",
  "reasonTip.parent_exited":
    "The recorded parent no longer exists in the process table — whatever launched it has exited.",
  "reasonTip.pid_slot_reused":
    "The parent PID points at a process created AFTER this one — the slot was recycled by an unrelated process; the real parent is dead.",
  "reasonTip.orphaned_chain":
    "The parent chain reaches the system root without passing any live user-visible app — the launching shell is dead (e.g. a closed terminal window).",
  "reasonTip.orphaned_session":
    "The process holds a real terminal (ttys) that no longer has a session leader — its terminal app likely crashed or was killed.",
  "reasonTip.nonstandard_path":
    "The executable is not in a standard install location (Applications, system dirs, Program Files).",
  "reasonTip.dev_server_keyword":
    "The command line matches a common dev-server keyword (vite / node / uvicorn …).",
  "reasonTip.duplicate_dev_server":
    "Another identical dev-server instance of the same project is already listening (port auto-incremented, or spread across terminals / IDEs) — usually a forgotten earlier launch. Keep the one you use and kill the other; star both if intentional.",
  "reasonTip.automation_instance":
    "The command line is that of a throwaway automation session (--headless plus a debugging port / temporary user-data dir) — a browser started by Playwright, Puppeteer, a scraping script and the like. It has outlived whatever started it and nothing is driving it any more (a client connected to its debugging port would clear it), so only this instance and its children keep spinning.",
  "reasonTip.debugger_attached":
    "A client is currently connected to its debugging port — something is driving this browser instance right now, so it is not residue. Killing it would interrupt a live session; never flagged.",
  "reasonTip.launchd_managed":
    "Claimed by launchctl (LaunchAgent / brew services) — intentionally supervised by launchd, not a zombie.",
  "reasonTip.brew_service_path":
    "Executable lives under a Homebrew service path (postgres / redis started by brew services) — not a zombie.",
  "reasonTip.installed_app":
    "Located in a standard install location (/Applications, system dirs, Program Files) — not a zombie.",
  "reasonTip.pm2_managed":
    "Supervised by the pm2 daemon — intentionally long-running, not a zombie.",
  "reasonTip.just_reparented":
    "Started (or reparented) less than 10 seconds ago — possibly a restart in flight. Listed as Possible; never swept.",

  "empty.none": "No listening ports found",
  "empty.noMatch": "No matches",
  "empty.scanning": "Scanning…",
  "empty.scanFailed": "That scan did not go through.",
  "empty.retry": "Retry",
  "empty.noStarred":
    "Nothing starred yet. A starred (★) process is never flagged as a zombie and never swept.",

  "kill.survivor": "PID {pid} {label} took the signal but has not exited.",
  "kill.survivor.force": "Force kill",
  "kill.survivor.dismiss": "Dismiss",

  "batch.title": "Sweep {n} suspected zombie processes",
  "batch.signal": "Signal",
  "batch.signal.macos": "SIGTERM (-15), graceful",
  "batch.signal.windows": "TerminateProcess (forced)",
  "batch.procs": "Processes",
  "batch.more": "… and {n} more",
  "batch.scrollHint": "{n} in total — scroll to see them all",
  "batch.scope.note":
    "Only 'Zombie' and 'Likely zombie' rows are swept; handle 'Possible' entries individually",
  "batch.cancel": "Cancel",
  "batch.confirm": "Terminate all",

  "confirm.title.kill": "Terminate process",
  "confirm.title.force": "Force-kill process",
  "confirm.app": "App",
  "confirm.cmd": "Command",
  "confirm.pid": "PID",
  "confirm.ports": "Ports",
  "confirm.portsRelease": "· killing this process frees all {n} ports",
  "confirm.signal": "Signal",
  "confirm.signal.term": "SIGTERM (-15), graceful",
  "confirm.signal.kill": "SIGKILL (-9), cannot be ignored",
  "confirm.signal.win": "TerminateProcess (immediate, forced)",
  "confirm.cancel": "Cancel",
  "confirm.kill": "Terminate",
  "confirm.force": "Force kill",
};

const dict = { zh, en };

export type Lang = "zh" | "en";

const STORAGE_KEY = "portreaper.lang";

function initialLang(): Lang {
  try {
    const saved = localStorage.getItem(STORAGE_KEY);
    if (saved === "zh" || saved === "en") return saved;
  } catch {
    /* localStorage 不可用时退回浏览器语言 */
  }
  return navigator.language.toLowerCase().startsWith("zh") ? "zh" : "en";
}

let current: Lang = initialLang();
const listeners = new Set<() => void>();

/** <html lang> 跟随界面语言（读屏发音 / 断词依赖它；index.html 的静态值只是占位） */
function syncDocumentLang(lang: Lang) {
  try {
    document.documentElement.lang = lang === "zh" ? "zh-CN" : "en";
  } catch {
    /* 非 DOM 环境忽略 */
  }
}

export function setLang(lang: Lang) {
  if (lang === current) return;
  current = lang;
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {
    /* 忽略持久化失败 */
  }
  syncDocumentLang(lang);
  // 同步托盘菜单 / tooltip 语言（失败静默：托盘不可用不影响主界面）
  invoke("set_tray_language", { lang }).catch(() => {});
  listeners.forEach((fn) => fn());
}

function getLang(): Lang {
  return current;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

function translate(lang: Lang, key: I18nKey, params?: Record<string, string | number>): string {
  // 兜底：动态 key（reason.* / story.* 经 as 断言传入）在 Rust 新增码而字典
  // 未更新的陈旧构建里可能缺失 —— 退回英文，再退回 key 本身，绝不渲染空文案。
  // （CI 的 check-reason-parity.mjs 保证正常构建不会走到兜底。）
  let s: string = dict[lang][key] ?? dict.en[key] ?? key;
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      s = s.split(`{${k}}`).join(String(v));
    }
  }
  return s;
}

/** 组件内使用：const { t, lang, setLang } = useI18n() */
export function useI18n() {
  const lang = useSyncExternalStore(subscribe, getLang);
  const t = (key: I18nKey, params?: Record<string, string | number>) =>
    translate(lang, key, params);
  return { t, lang, setLang };
}

// 应用启动时把检测到的语言同步给托盘与 <html lang>（与前端保持一致）
syncDocumentLang(current);
invoke("set_tray_language", { lang: current }).catch(() => {});
