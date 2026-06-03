// 轻量类型安全 i18n —— 不引库：
// - key 类型从 zh 字典推导，en 用 Record<Key, string> 约束 → 缺/多 key 都是 tsc 编译错误
// - reason.* / confidence.* 键与 Rust 端 ReasonCode/Confidence 的 serde 名一一对应，
//   CI 的 scripts/check-reason-parity.mjs 做机械校验
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

const zh = {
  // ---- header ----
  "pills.processes": "进程",
  "pills.ports": "端口",
  "pills.suspects": "疑似僵尸",
  "pills.whitelisted": "已收藏",
  "header.autoRefresh": "· 每 2s 自动刷新",

  // ---- toolbar ----
  "search.placeholder": "搜索 端口 / PID / App / 启动方",
  "filter.all": "全部",
  "filter.suspect": "仅疑似僵尸",
  "filter.whitelist": "已收藏",
  "sweep.button": "一键清扫",
  "sweep.sweeping": "清扫中…",
  "sweep.title": "批量终止「确认 + 很可能」级别的疑似僵尸（「存疑」不会被清扫）",
  "refresh.title": "立即刷新（自动每 2 秒会刷一次）",
  "refresh.aria": "立即刷新",

  // ---- errors ----
  "error.clickToClose": "· 点击关闭",
  "error.killFailed": "Kill 失败: {err}",
  "error.openBrowser": "打开浏览器失败: {err}",
  "error.batchFailed": "{failed}/{total} 个进程 kill 失败：",
  "error.pidReused": "进程身份已变化（PID 被复用），已取消终止，请重新扫描",
  "error.processGone": "进程已不存在（可能刚刚退出）",

  // ---- table headers ----
  "th.ports": "端口",
  "th.ports.tip":
    "进程正在 TCP LISTEN 的本地端口。点击端口 chip 会在浏览器打开 http://localhost:PORT",
  "th.type": "类型",
  "th.type.tip": "进程分类（脚本 / APP / 系统 / CLI）",
  "th.app": "App",
  "th.app.tip": "App 名称（由 Rust 端的 identify_app 算出的人类可读标签）",
  "th.path": "路径",
  "th.path.tip": "可执行文件的完整路径。鼠标悬停在路径上可查看完整命令行（含所有参数）",
  "th.launcher": "启动者",
  "th.launcher.tip": "沿父进程链向上找到的第一个用户可见 App；下方小字是中间的脚本/进程",
  "th.pid": "PID",
  "th.pid.tip.macos":
    "本进程 PID / 父进程 PID。父进程是 launchd (PID 1) 意味着原启动者已退出 —— 这是「孤儿」信号。",
  "th.pid.tip.windows":
    "本进程 PID / 父进程 PID。父进程已退出意味着原启动者（终端 / IDE）不在了 —— 这是「孤儿」信号。",
  "th.elapsed": "已运行",
  "th.elapsed.tip": "进程已运行时长（HH:MM:SS，超过 1 天显示为 dd-HH:MM:SS）",
  "th.cpu": "CPU",
  "th.cpu.tip": "进程占用 CPU 的百分比（瞬时采样，多线程可超过 100%）",
  "th.mem": "内存",
  "th.mem.tip": "进程常驻物理内存（RSS）",
  "th.actions": "操作",
  "th.actions.tip.macos":
    "Kill = SIGTERM 优雅终止；强杀 = SIGKILL 强制终止；★ 加入收藏后不再被判为僵尸",
  "th.actions.tip.windows":
    "终止 = TerminateProcess（Windows 无 SIGTERM，无法优雅终止脱离控制台的进程）；★ 加入收藏后不再被判为僵尸",

  // ---- cells ----
  "port.tip": "在浏览器打开 http://localhost:{port}",
  "args.label": "参数",
  "cmd.full.tip": "完整命令：{cmd}",
  "pid.self": "本进程",
  "pid.self.tip": "本进程 PID（检测到正在监听端口的进程）",
  "pid.parent": "父进程",
  "pid.parent.tip.launchd":
    "父进程 PID 1 = launchd —— macOS 的进程主控（开机后第一个启动，所有进程的最终祖先）。父进程为 launchd 通常意味着原启动者（终端 / IDE）已退出，本进程被「收养」为孤儿。",
  "pid.parent.tip.normal": "父进程 PID（启动本进程的进程）",
  "cpu.tip": "CPU 占用 {v}%",
  "mem.tip": "常驻物理内存 {v} MB",

  // ---- actions ----
  "kill.btn": "Kill",
  "kill.force.btn": "强杀",
  "kill.force.tip": "kill -9 强制",
  "kill.terminate.btn": "终止",
  "kill.terminate.tip": "TerminateProcess 立即终止（Windows 无优雅终止）",
  "star.add.tip": "加入收藏（不再被标记为僵尸）",
  "star.remove.tip": "从收藏移除",
  "whitelist.chip": "★ 收藏",

  // ---- categories ----
  "cat.installed-app": "APP",
  "cat.system": "系统",
  "cat.dev-script": "脚本",
  "cat.user-binary": "CLI",
  "cat.unknown": "?",

  // ---- confidence tiers（与 Rust Confidence serde 名对应）----
  "confidence.confirmed": "确认",
  "confidence.likely": "很可能",
  "confidence.possible": "存疑",

  // ---- reason codes（与 Rust ReasonCode serde 名对应）----
  "reason.defunct": "进程已死 (defunct)",
  "reason.ppid1_orphan": "孤儿 (PPID=1)",
  "reason.parent_exited": "父进程已退出",
  "reason.pid_slot_reused": "父 PID 已被复用",
  "reason.orphaned_chain": "孤儿进程链",
  "reason.orphaned_session": "终端会话已死",
  "reason.nonstandard_path": "非标准安装路径",
  "reason.dev_server_keyword": "dev-server 关键字",
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
  "reasonTip.launchd_managed":
    "launchctl 认领的任务（LaunchAgent / brew services）—— 由 launchd 有意托管，不是僵尸。",
  "reasonTip.brew_service_path":
    "可执行文件位于 Homebrew 服务路径（brew services 启动的 postgres / redis 等），不是僵尸。",
  "reasonTip.installed_app": "位于标准安装位置（/Applications、系统目录、Program Files 等），不是僵尸。",
  "reasonTip.pm2_managed": "由 pm2 守护进程托管 —— 用户有意让它常驻，不是僵尸。",
  "reasonTip.just_reparented": "启动（或刚被收养）不足 10 秒，可能正处于重启过渡态 —— 暂列存疑，不会被清扫。",

  "exempt.tip": "为什么未被标记：",

  // ---- empty states ----
  "empty.none": "没有发现任何监听端口",
  "empty.noMatch": "没有匹配项",

  // ---- batch modal ----
  "batch.title": "一键清扫 {n} 个疑似僵尸进程",
  "batch.signal": "信号",
  "batch.signal.macos": "SIGTERM (-15) 优雅终止",
  "batch.signal.windows": "TerminateProcess（强制终止）",
  "batch.procs": "进程",
  "batch.more": "… 还有 {n} 个",
  "batch.scope.note": "仅清扫「确认」与「很可能」级别；「存疑」需逐个手动处理",
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

const en: Record<I18nKey, string> = {
  "pills.processes": "processes",
  "pills.ports": "ports",
  "pills.suspects": "suspects",
  "pills.whitelisted": "starred",
  "header.autoRefresh": "· refreshes every 2s",

  "search.placeholder": "Search port / PID / app / launcher",
  "filter.all": "All",
  "filter.suspect": "Suspects only",
  "filter.whitelist": "Starred",
  "sweep.button": "Sweep",
  "sweep.sweeping": "Sweeping…",
  "sweep.title":
    "Batch-terminate Confirmed + Likely suspects (Possible is never swept)",
  "refresh.title": "Refresh now (auto-refreshes every 2 seconds)",
  "refresh.aria": "Refresh now",

  "error.clickToClose": "· click to dismiss",
  "error.killFailed": "Kill failed: {err}",
  "error.openBrowser": "Failed to open browser: {err}",
  "error.batchFailed": "{failed}/{total} processes failed to terminate:",
  "error.pidReused":
    "Process identity changed (PID was reused) — kill aborted, please rescan",
  "error.processGone": "Process no longer exists (it may have just exited)",

  "th.ports": "Ports",
  "th.ports.tip":
    "Local TCP LISTEN ports of this process. Click a port chip to open http://localhost:PORT",
  "th.type": "Type",
  "th.type.tip": "Process category (script / app / system / CLI)",
  "th.app": "App",
  "th.app.tip": "Human-readable label computed by identify_app on the Rust side",
  "th.path": "Path",
  "th.path.tip":
    "Full executable path. Hover to see the complete command line with all arguments",
  "th.launcher": "Launcher",
  "th.launcher.tip":
    "First user-visible app found walking up the parent chain; smaller lines are intermediate processes",
  "th.pid": "PID",
  "th.pid.tip.macos":
    "Process PID / parent PID. A parent of launchd (PID 1) means the original launcher exited — the orphan signal.",
  "th.pid.tip.windows":
    "Process PID / parent PID. A vanished parent means the original launcher (terminal / IDE) is gone — the orphan signal.",
  "th.elapsed": "Uptime",
  "th.elapsed.tip": "Elapsed run time (HH:MM:SS, or dd-HH:MM:SS beyond one day)",
  "th.cpu": "CPU",
  "th.cpu.tip": "CPU usage (instantaneous sample; can exceed 100% for multithreaded processes)",
  "th.mem": "Memory",
  "th.mem.tip": "Resident set size (RSS)",
  "th.actions": "Actions",
  "th.actions.tip.macos":
    "Kill = SIGTERM (graceful); Force = SIGKILL; ★ starred entries are never flagged as zombies",
  "th.actions.tip.windows":
    "Terminate = TerminateProcess (Windows has no SIGTERM for detached processes); ★ starred entries are never flagged",

  "port.tip": "Open http://localhost:{port} in browser",
  "args.label": "args",
  "cmd.full.tip": "Full command: {cmd}",
  "pid.self": "self",
  "pid.self.tip": "PID of the listening process",
  "pid.parent": "parent",
  "pid.parent.tip.launchd":
    "Parent PID 1 = launchd — macOS's process supervisor (first process at boot, ultimate ancestor of everything). A launchd parent usually means the original launcher (terminal / IDE) exited and this process was adopted as an orphan.",
  "pid.parent.tip.normal": "Parent PID (the process that started this one)",
  "cpu.tip": "CPU usage {v}%",
  "mem.tip": "Resident memory {v} MB",

  "kill.btn": "Kill",
  "kill.force.btn": "Force",
  "kill.force.tip": "kill -9 (SIGKILL)",
  "kill.terminate.btn": "Terminate",
  "kill.terminate.tip":
    "TerminateProcess — immediate (Windows has no graceful kill for detached processes)",
  "star.add.tip": "Star (never flag as zombie)",
  "star.remove.tip": "Remove star",
  "whitelist.chip": "★ starred",

  "cat.installed-app": "APP",
  "cat.system": "SYS",
  "cat.dev-script": "DEV",
  "cat.user-binary": "CLI",
  "cat.unknown": "?",

  "confidence.confirmed": "Confirmed",
  "confidence.likely": "Likely",
  "confidence.possible": "Possible",

  "reason.defunct": "defunct",
  "reason.ppid1_orphan": "orphan (PPID=1)",
  "reason.parent_exited": "parent exited",
  "reason.pid_slot_reused": "parent PID reused",
  "reason.orphaned_chain": "orphaned chain",
  "reason.orphaned_session": "dead terminal session",
  "reason.nonstandard_path": "non-standard path",
  "reason.dev_server_keyword": "dev-server keyword",
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

  "exempt.tip": "Why not flagged: ",

  "empty.none": "No listening ports found",
  "empty.noMatch": "No matches",

  "batch.title": "Sweep {n} suspected zombie processes",
  "batch.signal": "Signal",
  "batch.signal.macos": "SIGTERM (-15), graceful",
  "batch.signal.windows": "TerminateProcess (forced)",
  "batch.procs": "Processes",
  "batch.more": "… and {n} more",
  "batch.scope.note":
    "Only Confirmed and Likely tiers are swept; handle Possible entries individually",
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

export function setLang(lang: Lang) {
  if (lang === current) return;
  current = lang;
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {
    /* 忽略持久化失败 */
  }
  // 同步托盘菜单 / tooltip 语言（失败静默：托盘不可用不影响主界面）
  invoke("set_tray_language", { lang }).catch(() => {});
  listeners.forEach((fn) => fn());
}

export function getLang(): Lang {
  return current;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function translate(
  lang: Lang,
  key: I18nKey,
  params?: Record<string, string | number>,
): string {
  let s: string = dict[lang][key];
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

// 应用启动时把系统检测到的语言同步给托盘（与前端保持一致）
invoke("set_tray_language", { lang: current }).catch(() => {});
