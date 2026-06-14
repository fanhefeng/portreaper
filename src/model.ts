// 纯逻辑层（无 React / 无 Tauri 依赖）：类型契约镜像 + 工具函数。
// 从 App.tsx 拆出（评审发现：纯函数不导出导致只能整棵渲染着测），
// 单测可直接 import，App.tsx 只留组件。
import type { I18nKey } from "./i18n";

export type ParentRef = {
  pid: number;
  label: string;
  category: string;
  exe_path: string;
};

export type Confidence = "none" | "possible" | "likely" | "confirmed";

/** Rust scanner::model::ProcessEntry 的 serde 契约镜像（字段一一对应） */
export type ProcessEntry = {
  pid: number;
  ppid: number;
  ports: number[];
  command: string;
  full_command: string;
  exe_path: string;
  app_label: string;
  app_category: string;
  parent_chain: ParentRef[];
  launcher_label: string;
  user: string;
  tty: string;
  elapsed_secs: number;
  start_unix: number | null;
  cpu_percent: number;
  mem_mb: number;
  state: string;
  is_zombie_suspect: boolean;
  confidence: Confidence;
  zombie_reasons: string[];
  is_whitelisted: boolean;
  duplicate_of: number | null;
};

export type Os = "macos" | "windows";

export type Filter = "all" | "suspect" | "whitelist";

/** t() 的函数签名（useI18n 返回值），供纯函数层接受翻译器注入 */
export type Translator = (
  k: I18nKey,
  p?: Record<string, string | number>,
) => string;

/** 一键清理覆盖的置信层级（Possible 永不入清扫） */
export const SWEEPABLE: ReadonlySet<Confidence> = new Set(["confirmed", "likely"]);

/** 会被一键清扫覆盖的行（嫌疑 + 置信层级达标）—— 托盘计数与清扫列表共用，
 *  避免两处各写一遍同一过滤条件而漂移（评审发现）。 */
export function sweepableEntries(entries: ProcessEntry[]): ProcessEntry[] {
  return entries.filter((e) => e.is_zombie_suspect && SWEEPABLE.has(e.confidence));
}

/** 豁免类 reason code —— 非嫌疑行的详情里以「为什么不是僵尸」展示 */
export const EXEMPT_REASONS = new Set([
  "launchd_managed",
  "brew_service_path",
  "installed_app",
  "pm2_managed",
]);

/** 非嫌疑行的豁免原因（「为什么不是僵尸」）—— Row 与 Detail 详情共用。
 *  原先两处各写一遍同一过滤（评审发现）。 */
export function exemptReasons(e: ProcessEntry): string[] {
  return e.is_zombie_suspect
    ? []
    : e.zombie_reasons.filter((r) => EXEMPT_REASONS.has(r));
}

/** 行内只讲一个最重要的原因（其余进详情面板），此为优先级 */
export const REASON_PRIORITY = [
  "defunct",
  "ppid1_orphan",
  "orphaned_chain",
  "parent_exited",
  "pid_slot_reused",
  "orphaned_session",
  "duplicate_dev_server",
  "just_reparented",
  "nonstandard_path",
  "dev_server_keyword",
];

export function primaryReason(reasons: string[]): string | null {
  for (const r of REASON_PRIORITY) if (reasons.includes(r)) return r;
  return reasons[0] ?? null;
}

/**
 * 白名单键 —— 必须与后端 scanner::mod::whitelist_key 逐字一致（评审发现）：
 * exe_path 含路径分隔符（绝对路径）时用它；否则是 PATH 解析的裸解释器名
 * （"node"），单独加白会塌缩匹配全机同名监听者 —— 回退完整命令行。
 */
export function whitelistKey(e: ProcessEntry): string {
  if (e.exe_path.includes("/") || e.exe_path.includes("\\")) return e.exe_path;
  return e.full_command || e.command;
}

/**
 * v0.4.0 旧键（exe_path 非空即用，否则短名）—— 后端 scanner::mod::legacy_whitelist_key
 * 的逐字镜像。升级兼容：取消加白时需连旧键一并删除，否则 v0.4.0 存的裸键仍会命中、
 * 星标取消不掉（评审发现）。
 */
export function legacyWhitelistKey(e: ProcessEntry): string {
  return e.exe_path || e.command;
}

/** 粗粒度运行时长（精确值在详情面板） */
export function formatUptime(secs: number, t: Translator): string {
  if (secs < 60) return t("uptime.now");
  if (secs < 3600) return t("uptime.min", { n: Math.floor(secs / 60) });
  if (secs < 86400) return t("uptime.hour", { n: Math.floor(secs / 3600) });
  return t("uptime.day", { n: Math.floor(secs / 86400) });
}

export function formatDuration(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return d > 0
    ? `${d}-${pad(h)}:${pad(m)}:${pad(s)}`
    : `${pad(h)}:${pad(m)}:${pad(s)}`;
}

/** 端口列表 → ":3000 :3001" 串（sep 默认单空格；弹窗等紧凑处传 "  "）。
 *  原先 App.tsx 四处各写一遍 ports.map(...).join(...)（评审发现）。 */
export function formatPorts(ports: number[], sep = " "): string {
  return ports.map((p) => `:${p}`).join(sep);
}

/** 后端语义错误（ERR_* 前缀）→ 本地化文案；其余透传 OS 原文 */
export function localizeKillError(err: string, t: Translator): string {
  if (err.includes("ERR_PID_REUSED")) return t("error.pidReused");
  if (err.includes("ERR_PROCESS_GONE")) return t("error.processGone");
  if (err.includes("ERR_ACCESS_DENIED")) return t("error.accessDenied");
  if (err.includes("ERR_IDENTITY_UNKNOWN")) return t("error.identityUnknown");
  return err;
}

/** scan_ports 无取消机制：后端子进程（lsof/launchctl）若卡死，invoke 会永不
 *  settle，inFlight 永久占用 → 之后每次轮询都早返回，UI 静默冻结在旧数据且无
 *  错误提示（评审发现）。用超时把它转成可见的 scanError + 下一轮自动重试。 */
export const SCAN_TIMEOUT_MS = 10_000;

export function withTimeout<T>(p: Promise<T>, ms: number, marker: string): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => reject(new Error(marker)), ms);
  });
  return Promise.race([p, timeout]).finally(() => clearTimeout(timer));
}

/** 扫描超时 sentinel → 本地化；其余（OS 原文）透传 */
export function localizeScanError(err: string, t: Translator): string {
  if (err.includes("ERR_SCAN_TIMEOUT")) return t("error.scanTimeout");
  return err;
}
