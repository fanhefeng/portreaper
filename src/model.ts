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
  /** 自身 + 全部后代的 CPU 合计（展示用，不参与判定）——
   *  headless 浏览器把 CPU 烧在 gpu-process 子进程里，主进程行看着是闲的 */
  cpu_percent_tree: number;
  mem_mb: number;
  state: string;
  is_zombie_suspect: boolean;
  confidence: Confidence;
  zombie_reasons: string[];
  is_whitelisted: boolean;
  /**
   * 白名单键，由引擎直接产出（Rust `scanner::whitelist_key`）—— **直接读它，
   * 绝不在 TS 里重推**。推导规则有条反直觉的分支（`exe_path` 仅在含路径分隔符时
   * 可用，否则回退全命令行：`ps -o comm=` 对 PATH 解析出的裸解释器名只返回
   * `node`，拿它当键会把全机同名监听者一起加白）。本文件一度重写过一遍那条规则，
   * 于是要靠单测钉住两份实现一致；改为读本字段后，「在 Raycast 加的星标桌面版
   * 认不出来」这类 bug 从结构上不可能发生。Raycast 侧同一约定见
   * `integrations/raycast/src/cli.ts whitelistKey`。
   */
  whitelist_key: string;
  duplicate_of: number | null;
};

export type Os = "macos" | "windows";

export type Filter = "all" | "suspect" | "whitelist";

/** t() 的函数签名（useI18n 返回值），供纯函数层接受翻译器注入 */
export type Translator = (k: I18nKey, p?: Record<string, string | number>) => string;

/** 一键清理覆盖的置信层级（Possible 永不入清扫） */
const SWEEPABLE: ReadonlySet<Confidence> = new Set(["confirmed", "likely"]);

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
  "debugger_attached",
]);

/** 非嫌疑行的豁免原因（「为什么不是僵尸」）—— Row 与 Detail 详情共用。
 *  原先两处各写一遍同一过滤（评审发现）。 */
export function exemptReasons(e: ProcessEntry): string[] {
  return e.is_zombie_suspect ? [] : e.zombie_reasons.filter((r) => EXEMPT_REASONS.has(r));
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
  // 排在两条泛化信号之前：「自动化会话残留」比「非标准路径 / dev 关键字」
  // 具体得多，是这类行最该讲给用户听的那句话
  "automation_instance",
  "nonstandard_path",
  "dev_server_keyword",
];

export function primaryReason(reasons: string[]): string | null {
  for (const r of REASON_PRIORITY) if (reasons.includes(r)) return r;
  return reasons[0] ?? null;
}

/**
 * v0.4.0 旧键（exe_path 非空即用，否则短名）—— 后端 scanner::mod::legacy_whitelist_key
 * 的逐字镜像。升级兼容：取消加白时需连旧键一并删除，否则 v0.4.0 存的裸键仍会命中、
 * 星标取消不掉（评审发现）。
 */
export function legacyWhitelistKey(e: ProcessEntry): string {
  return e.exe_path || e.command;
}

/** 子树 CPU 明显高于本行自身（后端 fill_subtree_cpu 的合计值）。
 *  1 个百分点的死区避开采样抖动；字段缺失（陈旧后端 / 测试夹具）时优雅退化为 false。 */
export function subtreeCpuExceedsSelf(e: ProcessEntry): boolean {
  return (e.cpu_percent_tree ?? 0) > e.cpu_percent + 1;
}

/** 行内「负载烧在子进程里」徽标的门槛（单核百分比）。
 *  用意（KNOWN-GAPS Gap 1/B）：无头浏览器主进程显示 ~0%，真凶是它的
 *  gpu-process 子进程 —— 这条反直觉的情形才值得占行内的位置。
 *  自身就在满核的健康构建（vite build / tsc）不满足「高于自身」，不会挂徽标。 */
export const BUSY_SUBTREE_PERCENT = 50;

export function hasBusySubtree(e: ProcessEntry): boolean {
  return subtreeCpuExceedsSelf(e) && e.cpu_percent_tree >= BUSY_SUBTREE_PERCENT;
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
  return d > 0 ? `${d}-${pad(h)}:${pad(m)}:${pad(s)}` : `${pad(h)}:${pad(m)}:${pad(s)}`;
}

/** 端口列表 → ":3000 :3001" 串（sep 默认单空格；弹窗等紧凑处传 "  "）。
 *  原先 App.tsx 四处各写一遍 ports.map(...).join(...)（评审发现）。 */
export function formatPorts(ports: number[], sep = " "): string {
  return ports.map((p) => `:${p}`).join(sep);
}

/**
 * 展示用的命令行：优先完整命令行，为空时退回 lsof 短名。
 *
 * 「显示哪一个」是展示契约的一部分（毁灭性确认要让用户看清杀的是什么，短名
 * `node` 不够辨认），此前六处各写一遍 `e.full_command || e.command`（评审发现）——
 * 与 `formatPorts` 收敛四处 join 是同一理由：漏改一处只会在个别界面上表现为
 * 不一致，没有任何东西会报错。取 `Pick` 而非整个 `ProcessEntry`，确认框那类
 * 只携带部分字段的快照也能直接用。
 */
export function displayCommand(e: Pick<ProcessEntry, "full_command" | "command">): string {
  return e.full_command || e.command;
}

/** 变更类 invoke（kill / 白名单）共用的超时与本地化：与 SCAN_TIMEOUT_MS 同一
 *  故障类（后端子进程挂起 → invoke 永不 settle），但后果更糟 —— runAction 的
 *  finally 永不执行，sweeping/killingPid 卡死会**永久禁用**清扫和行内按钮且无
 *  任何报错（评审发现：scan 修了、mutation 漏了的不对称）。kill 侧 shell 出
 *  ps + kill 两个子进程，给比扫描更宽的余量。 */
export const ACTION_TIMEOUT_MS = 15_000;

export function localizeActionError(err: string, t: Translator): string {
  if (err.includes("ERR_ACTION_TIMEOUT")) return t("error.actionTimeout");
  return err;
}

/** 引擎 `KillError` 的 serde 镜像（`platform.rs`，`#[serde(tag = "code")]`）。
 *  桌面与 Raycast 吃的是**同一个值**：Tauri 直接返回它，CLI 把它 JSON 到 stderr。
 *  新增变体时这里加一支，下面的 switch 立刻编译期报错 —— 这正是取代
 *  `includes("ERR_…")` 的理由：字符串匹配漏改只会在运行时静默退化。 */
export type KillError =
  | { code: "identity_unknown" }
  | { code: "process_gone" }
  | { code: "pid_reused" }
  | { code: "access_denied" }
  | { code: "os"; message: string };

/** invoke 的 reject 值是 `unknown`：可能是 KillError，也可能是 withTimeout 抛的
 *  Error sentinel，或（IPC 层自身故障时）任意值。只认结构完整的那一种。 */
function asKillError(err: unknown): KillError | null {
  if (typeof err !== "object" || err === null || !("code" in err)) return null;
  const { code } = err as { code: unknown };
  switch (code) {
    case "identity_unknown":
    case "process_gone":
    case "pid_reused":
    case "access_denied":
      return { code };
    case "os": {
      const { message } = err as { message?: unknown };
      return { code, message: typeof message === "string" ? message : "" };
    }
    default:
      return null;
  }
}

/**
 * 判断「两次扫描里的这一行是不是同一个进程」时，`start_unix` 允许的偏差（秒）。
 *
 * **绝不能用 `===`**：macOS 的 `start_unix` 是 `now - etime` 推导出来的，而 `etime`
 * 只有秒级粒度 —— 同一个进程在连续两轮扫描里读到的值会 ±1s 抖动（本机实测：
 * 14 轮采样，13 个进程**全部**出现 1 秒极差）。用严格相等做存活确认，会把
 * 「进程还在」随机读成「进程已消失」，正好废掉终止后确认这件事。
 *
 * 取值与引擎的 `START_TOLERANCE_SECS` 一致，理由也一致：被复用的 PID 其创建时间
 * 必然晚于扫描时刻，远超这个容差，不会被误判成同一个进程。
 */
export const START_MATCH_TOLERANCE_SECS = 5;

/** 两次扫描里的行是否指向同一个进程（PID + 创建时间，带容差）。 */
export function isSameProcess(e: ProcessEntry, pid: number, startUnix: number | null): boolean {
  if (e.pid !== pid) return false;
  if (startUnix == null || e.start_unix == null) return true; // 没有令牌可比，只能认 PID
  return Math.abs(e.start_unix - startUnix) <= START_MATCH_TOLERANCE_SECS;
}

/** 结构化失败的语义码；不是已知形态时 null。
 *
 *  给**流程分叉**用（例如「目标在点击前已自行退出」不该弹红条），文案仍走
 *  `localizeKillError`。**绝不能**退化成对错误文本的子串匹配 —— v0.9.0 删掉
 *  `ERR_*:` 前缀契约要根除的正是那个。 */
export function killErrorCode(err: unknown): KillError["code"] | null {
  return asKillError(err)?.code ?? null;
}

/** 对象形态但 code 不认识（后端比前端新，例如用户没升桌面端就换了 CLI）：
 *  `String(err)` 会渲染成 `[object Object]` —— 比旧的字符串契约还糟，用户拿不到
 *  任何可搜索、可报 issue 的信息。故降级也要吐出 code 本身。 */
function unknownErrorText(err: unknown): string | null {
  if (typeof err !== "object" || err === null || !("code" in err)) return null;
  const { code, message } = err as { code?: unknown; message?: unknown };
  if (typeof code !== "string") return null;
  return typeof message === "string" && message ? `${code}: ${message}` : code;
}

/** 后端语义错误（结构化 `{code}`）→ 本地化文案；OS 原文与超时 sentinel 透传。 */
export function localizeKillError(err: unknown, t: Translator): string {
  const killError = asKillError(err);
  if (killError) {
    switch (killError.code) {
      case "pid_reused":
        return t("error.pidReused");
      case "process_gone":
        return t("error.processGone");
      case "access_denied":
        return t("error.accessDenied");
      case "identity_unknown":
        return t("error.identityUnknown");
      case "os":
        // 无语义，原样展示。message 空则退回 code，绝不吐一个空横幅
        return killError.message || killError.code;
    }
  }
  return unknownErrorText(err) ?? localizeActionError(String(err), t);
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

/** 扫描侧语义错误（超时 sentinel / 后端拒扫）→ 本地化；其余（OS 原文）透传 */
export function localizeScanError(err: string, t: Translator): string {
  if (err.includes("ERR_SCAN_TIMEOUT")) return t("error.scanTimeout");
  if (err.includes("ERR_SCAN_BUSY")) return t("error.scanBusy");
  return err;
}
