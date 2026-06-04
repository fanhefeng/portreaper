import { useCallback, useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n, type I18nKey, type Lang } from "./i18n";
import "./App.css";

type ParentRef = {
  pid: number;
  label: string;
  category: string;
  exe_path: string;
};

type Confidence = "none" | "possible" | "likely" | "confirmed";

type ProcessEntry = {
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
};

type Os = "macos" | "windows";

type Filter = "all" | "suspect" | "whitelist";

type ConfirmState = {
  pid: number;
  ports: number[];
  command: string;
  app_label: string;
  force: boolean;
  startUnix: number | null;
} | null;

/** 一键清理覆盖的置信层级（Possible 永不入清扫） */
const SWEEPABLE: ReadonlySet<Confidence> = new Set(["confirmed", "likely"]);

/** 豁免类 reason code —— 非嫌疑行的详情里以「为什么不是僵尸」展示 */
const EXEMPT_REASONS = new Set([
  "launchd_managed",
  "brew_service_path",
  "installed_app",
  "pm2_managed",
]);

/** 行内只讲一个最重要的原因（其余进详情面板），此为优先级 */
const REASON_PRIORITY = [
  "defunct",
  "ppid1_orphan",
  "orphaned_chain",
  "parent_exited",
  "pid_slot_reused",
  "orphaned_session",
  "just_reparented",
  "nonstandard_path",
  "dev_server_keyword",
];

function primaryReason(reasons: string[]): string | null {
  for (const r of REASON_PRIORITY) if (reasons.includes(r)) return r;
  return reasons[0] ?? null;
}

/**
 * 常见进程知识库：让非技术用户一眼知道「这是什么软件、干什么用的」。
 * 顺序即优先级（具体的在前，node/python 等泛化的在后）；
 * 未命中时退回 desc.<category> 的类别描述。
 */
const KNOWN_PROCESSES: ReadonlyArray<readonly [RegExp, string, string]> = [
  // —— 开发服务器 / 框架（先于泛化的 node/python）——
  [/vite/, "Vite 前端开发服务器", "Vite frontend dev server"],
  [/webpack/, "Webpack 前端开发服务", "Webpack dev server"],
  [/next dev|next-server|next start/, "Next.js 开发服务器", "Next.js dev server"],
  [/nuxt/, "Nuxt 开发服务器", "Nuxt dev server"],
  [/uvicorn|gunicorn|fastapi|flask|django/, "Python Web 服务", "Python web service"],
  [/http\.server/, "Python 临时文件服务器", "Python ad-hoc file server"],
  [/jupyter/, "Jupyter 笔记本服务", "Jupyter notebook server"],
  [/storybook/, "Storybook 组件预览服务", "Storybook preview server"],
  // —— 数据库 / 服务 ——
  [/postgres/, "PostgreSQL 数据库", "PostgreSQL database"],
  [/mysqld|mariadb/, "MySQL 数据库", "MySQL database"],
  [/mongod/, "MongoDB 数据库", "MongoDB database"],
  [/\bredis\b/, "Redis 数据库", "Redis database"],
  [/nginx/, "Nginx Web 服务器", "Nginx web server"],
  [/caddy/, "Caddy Web 服务器", "Caddy web server"],
  [/docker|containerd/, "Docker 容器服务", "Docker container service"],
  [/ollama/, "Ollama 本地 AI 模型服务", "Ollama local AI model server"],
  // —— 常见桌面软件（带 \b 词界防止子串误匹配，如路径里恰好含 "php"）——
  [/wechat|weixin/, "微信", "WeChat messenger"],
  [/wxwork|wework/, "企业微信", "WeCom"],
  [/qqmusic/, "QQ 音乐", "QQ Music"],
  [/\bqq\b/, "QQ", "QQ messenger"],
  [/dingtalk/, "钉钉", "DingTalk"],
  [/feishu|\blark\b/, "飞书", "Lark / Feishu"],
  [/cloudmusic|neteasemusic/, "网易云音乐", "NetEase Cloud Music"],
  [/wemeet|tencentmeeting/, "腾讯会议", "Tencent Meeting"],
  [/todesk/, "ToDesk 远程控制", "ToDesk remote desktop"],
  [/clash|v2ray|xray|sing-box|shadowsocks|trojan/, "网络代理工具", "network proxy tool"],
  [/raycast/, "Raycast 快捷启动工具", "Raycast launcher"],
  [/alfred/, "Alfred 快捷启动工具", "Alfred launcher"],
  [/\bspotify\b/, "Spotify 音乐", "Spotify music"],
  [/\bsteam\b/, "Steam 游戏平台", "Steam gaming platform"],
  [/onedrive/, "OneDrive 网盘同步", "OneDrive sync"],
  [/dropbox/, "Dropbox 网盘同步", "Dropbox sync"],
  [/baidunetdisk/, "百度网盘", "Baidu Netdisk"],
  [/code helper|visual studio code/, "VS Code 代码编辑器", "VS Code editor"],
  [/\bcursor\b/, "Cursor 代码编辑器", "Cursor editor"],
  [/iterm/, "iTerm 终端", "iTerm terminal"],
  [/\bwarp\b/, "Warp 终端", "Warp terminal"],
  // —— macOS 系统组件 ——
  [/controlcenter/, "macOS 控制中心（系统组件）", "macOS Control Center (system)"],
  [/rapportd/, "苹果设备互联服务（接力 / 隔空）", "Apple continuity service"],
  [/sharingd/, "macOS 共享服务", "macOS sharing service"],
  [/airplay/, "隔空播放服务", "AirPlay service"],
  // —— 泛化运行时（永远放最后；\b 词界防止把无关二进制误标）——
  [/cargo|target\/(debug|release)/, "Rust 开发程序", "Rust dev program"],
  [/\bnode\b|\bnpm\b|\bpnpm\b|\byarn\b|\bbun\b/, "Node.js 程序", "Node.js program"],
  [/\bpython/, "Python 程序", "Python program"],
  [/\bjava\b|gradle|tomcat/, "Java 程序", "Java program"],
  [/\bruby\b|\brails\b/, "Ruby 程序", "Ruby program"],
  [/\bphp\b/, "PHP 程序", "PHP program"],
];

/** 类别 → 兜底描述 key（知识库未命中时） */
const DESC_KEYS: Record<string, I18nKey> = {
  "installed-app": "desc.installed-app",
  system: "desc.system",
  "dev-script": "desc.dev-script",
  "user-binary": "desc.user-binary",
  unknown: "desc.unknown",
};

/** 「这是什么」：知识库命中 → 友好名；未命中 → 类别描述兜底 */
function describeEntry(e: ProcessEntry, lang: Lang): string | null {
  // full_command 必须参与匹配：lsof 的短名只有 "node"，脚本身份在完整命令行里
  const hay =
    `${e.app_label} ${e.command} ${e.full_command} ${e.exe_path}`.toLowerCase();
  for (const [re, zh, enText] of KNOWN_PROCESSES) {
    if (re.test(hay)) return lang === "zh" ? zh : enText;
  }
  return null;
}

/** 粗粒度运行时长（精确值在详情面板） */
function formatUptime(
  secs: number,
  t: (k: I18nKey, p?: Record<string, string | number>) => string,
): string {
  if (secs < 60) return t("uptime.now");
  if (secs < 3600) return t("uptime.min", { n: Math.floor(secs / 60) });
  if (secs < 86400) return t("uptime.hour", { n: Math.floor(secs / 3600) });
  return t("uptime.day", { n: Math.floor(secs / 86400) });
}

function formatDuration(secs: number): string {
  const d = Math.floor(secs / 86400);
  const h = Math.floor((secs % 86400) / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = secs % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  return d > 0
    ? `${d}-${pad(h)}:${pad(m)}:${pad(s)}`
    : `${pad(h)}:${pad(m)}:${pad(s)}`;
}

/** 极简焦点圈定：Tab 在弹窗内的按钮间循环（毁灭性确认弹窗的键盘安全网） */
function trapTab(ev: ReactKeyboardEvent<HTMLDivElement>) {
  if (ev.key !== "Tab") return;
  const btns = ev.currentTarget.querySelectorAll<HTMLButtonElement>("button");
  if (btns.length === 0) return;
  const first = btns[0];
  const last = btns[btns.length - 1];
  const active = document.activeElement;
  const inside = ev.currentTarget.contains(active);
  if (ev.shiftKey) {
    if (active === first || !inside) {
      ev.preventDefault();
      last.focus();
    }
  } else if (active === last || !inside) {
    ev.preventDefault();
    first.focus();
  }
}

/** 后端语义错误（ERR_* 前缀）→ 本地化文案；其余透传 OS 原文 */
function localizeKillError(
  err: string,
  t: (k: I18nKey, p?: Record<string, string | number>) => string,
): string {
  if (err.includes("ERR_PID_REUSED")) return t("error.pidReused");
  if (err.includes("ERR_PROCESS_GONE")) return t("error.processGone");
  if (err.includes("ERR_IDENTITY_UNKNOWN")) return t("error.identityUnknown");
  return err;
}

function App() {
  const { t, lang, setLang } = useI18n();
  const [os, setOs] = useState<Os>("macos");
  const [entries, setEntries] = useState<ProcessEntry[]>([]);
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [killingPid, setKillingPid] = useState<number | null>(null);
  const [confirm, setConfirm] = useState<ConfirmState>(null);
  const [batchConfirm, setBatchConfirm] = useState<ProcessEntry[] | null>(null);
  const [sweeping, setSweeping] = useState(false);
  const [expandedPid, setExpandedPid] = useState<number | null>(null);
  const inFlight = useRef<Promise<void> | null>(null);

  useEffect(() => {
    invoke<Os>("get_platform")
      .then(setOs)
      .catch(() => {});
  }, []);

  const runScan = useCallback(async () => {
    try {
      const data = await invoke<ProcessEntry[]>("scan_ports");
      setEntries(data);
      setError(null);
      // 托盘只计入会被清扫的层级（Confirmed + Likely），避免宽限期内的闪烁。
      // 托盘更新是装饰性的 —— fire-and-forget（与 i18n.ts 的 set_tray_language
      // 同一约定），绝不能让它把一次成功的扫描标成错误（评审发现）。
      const suspectCount = data.filter(
        (e) => e.is_zombie_suspect && SWEEPABLE.has(e.confidence),
      ).length;
      const totalPorts = data.reduce((sum, e) => sum + e.ports.length, 0);
      invoke("update_tray_title", {
        count: totalPorts,
        suspectCount,
      }).catch(() => {});
    } catch (e) {
      setError(String(e));
    }
  }, []);

  /** 轮询入口：已有扫描在跑则复用它的 Promise（防并发扫描） */
  const refresh = useCallback(() => {
    if (!inFlight.current) {
      inFlight.current = runScan().finally(() => {
        inFlight.current = null;
      });
    }
    return inFlight.current;
  }, [runScan]);

  /** kill 之后用：先等正在跑的扫描收尾，再扫一次，确保拿到 kill 后的真实状态
   *（runScan 内部消化所有异常，这里的 await 不会抛） */
  const freshScan = useCallback(async () => {
    if (inFlight.current) await inFlight.current;
    return refresh();
  }, [refresh]);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 2000);
    return () => clearInterval(id);
  }, [refresh]);

  // Esc 关闭弹窗 / 收起详情
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape") return;
      if (confirm) setConfirm(null);
      else if (batchConfirm) setBatchConfirm(null);
      else setExpandedPid(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [confirm, batchConfirm]);

  const suspectCount = entries.filter((e) => e.is_zombie_suspect).length;
  const sweepables = entries.filter(
    (e) => e.is_zombie_suspect && SWEEPABLE.has(e.confidence),
  );
  const whitelistCount = entries.filter((e) => e.is_whitelisted).length;
  const totalPortCount = entries.reduce((sum, e) => sum + e.ports.length, 0);

  const filtered = entries.filter((e) => {
    if (filter === "suspect" && !e.is_zombie_suspect) return false;
    if (filter === "whitelist" && !e.is_whitelisted) return false;
    if (search) {
      const q = search.toLowerCase();
      return (
        e.command.toLowerCase().includes(q) ||
        e.full_command.toLowerCase().includes(q) ||
        e.app_label.toLowerCase().includes(q) ||
        e.launcher_label.toLowerCase().includes(q) ||
        e.ports.some((p) => String(p).includes(q)) ||
        String(e.pid).includes(q)
      );
    }
    return true;
  });

  const suspects = filtered.filter((e) => e.is_zombie_suspect);
  const healthy = filtered.filter((e) => !e.is_zombie_suspect);

  const askKill = (e: ProcessEntry, force: boolean) => {
    setConfirm({
      pid: e.pid,
      ports: e.ports,
      command: e.command,
      app_label: e.app_label,
      force,
      startUnix: e.start_unix,
    });
  };

  const doKill = async () => {
    if (!confirm) return;
    const { pid, force, startUnix } = confirm;
    setKillingPid(pid);
    setConfirm(null);
    try {
      await invoke("kill_process", { pid, force, startUnix });
      await new Promise((r) => setTimeout(r, 250));
      await freshScan();
    } catch (err) {
      setError(t("error.killFailed", { err: localizeKillError(String(err), t) }));
    } finally {
      setKillingPid(null);
    }
  };

  const handleToggleWhitelist = async (e: ProcessEntry) => {
    const key = e.exe_path || e.command;
    try {
      if (e.is_whitelisted) {
        await invoke("remove_whitelist", { key });
      } else {
        await invoke("add_whitelist", { key });
      }
      await refresh();
    } catch (err) {
      setError(String(err));
    }
  };

  const handleOpen = async (port: number) => {
    try {
      await openUrl(`http://localhost:${port}`);
    } catch (err) {
      setError(t("error.openBrowser", { err: String(err) }));
    }
  };

  const askKillAllSuspects = () => {
    if (sweepables.length === 0) return;
    setBatchConfirm(sweepables);
  };

  const doBatchKill = async () => {
    const suspects = batchConfirm;
    if (!suspects || suspects.length === 0) return;
    setBatchConfirm(null);
    setSweeping(true);
    // Windows 无 SIGTERM：单一 TerminateProcess 语义（force）
    const force = os === "windows";
    const failures: { pid: number; label: string; err: string }[] = [];
    for (const s of suspects) {
      try {
        await invoke("kill_process", {
          pid: s.pid,
          force,
          startUnix: s.start_unix,
        });
      } catch (err) {
        failures.push({
          pid: s.pid,
          label: s.app_label,
          err: localizeKillError(String(err), t),
        });
      }
    }
    await new Promise((r) => setTimeout(r, 700));
    await freshScan(); // 等掉撞车的轮询再扫一次，结果必然包含 kill 之后的状态
    setSweeping(false);
    if (failures.length > 0) {
      setError(
        t("error.batchFailed", {
          failed: failures.length,
          total: suspects.length,
        }) +
          failures.map((f) => `PID ${f.pid} ${f.label} (${f.err})`).join("；"),
      );
    }
  };

  const toggleExpand = (pid: number) =>
    setExpandedPid((cur) => (cur === pid ? null : pid));

  const rowProps = {
    os,
    lang,
    killingPid,
    onAskKill: askKill,
    onToggleWhitelist: handleToggleWhitelist,
    onOpenPort: handleOpen,
    onToggleExpand: toggleExpand,
  };

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <svg
            className="brand-icon"
            viewBox="0 0 24 24"
            width="20"
            height="20"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M3 7l9 5 9-5" />
            <path d="M3 7v10l9 5 9-5V7" />
            <path d="M12 12v10" />
          </svg>
          <h1>Portreaper</h1>
        </div>

        <div className="search-wrap">
          <svg
            viewBox="0 0 24 24"
            width="14"
            height="14"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <circle cx="11" cy="11" r="7" />
            <line x1="21" y1="21" x2="16.65" y2="16.65" />
          </svg>
          <input
            type="text"
            placeholder={t("search.placeholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="search"
          />
        </div>

        <div className="filter-tabs">
          <button
            className={filter === "all" ? "active" : ""}
            onClick={() => setFilter("all")}
          >
            {t("filter.all")}
            <span className="tab-count">{entries.length}</span>
          </button>
          <button
            className={`${filter === "suspect" ? "active" : ""} ${suspectCount > 0 ? "has-suspects" : ""}`}
            onClick={() => setFilter("suspect")}
          >
            {t("filter.suspect")}
            <span className="tab-count">{suspectCount}</span>
          </button>
          <button
            className={filter === "whitelist" ? "active" : ""}
            onClick={() => setFilter("whitelist")}
          >
            {t("filter.whitelist")}
            <span className="tab-count">{whitelistCount}</span>
          </button>
        </div>

        <div className="header-right">
          {sweepables.length > 0 && (
            <button
              className="btn-sweep"
              disabled={sweeping}
              onClick={askKillAllSuspects}
              title={t("sweep.title")}
            >
              {sweeping
                ? t("sweep.sweeping")
                : `${t("sweep.button")} (${sweepables.length})`}
            </button>
          )}
          <button
            className="lang-toggle"
            onClick={() => setLang(lang === "zh" ? "en" : "zh")}
            title={lang === "zh" ? "Switch to English" : "切换为中文"}
          >
            {lang === "zh" ? "EN" : "中"}
          </button>
        </div>
      </header>

      {error && (
        <div className="error" onClick={() => setError(null)}>
          {error} {t("error.clickToClose")}
        </div>
      )}

      <main className="list">
        {entries.length === 0 ? (
          <div className="empty">{t("empty.none")}</div>
        ) : filtered.length === 0 ? (
          <div className="empty">{t("empty.noMatch")}</div>
        ) : (
          <>
            {filter !== "whitelist" && suspects.length > 0 && (
              <section>
                <div className="section-head section-head-danger">
                  <span className="section-title">{t("section.suspects")}</span>
                  <span className="section-count">{suspects.length}</span>
                  <span className="section-sub">{t("section.suspects.sub")}</span>
                </div>
                {suspects.map((e) => (
                  <Row
                    key={e.pid}
                    e={e}
                    expanded={expandedPid === e.pid}
                    {...rowProps}
                  />
                ))}
              </section>
            )}

            {filter !== "whitelist" && suspects.length === 0 && (
              <div className="allclear">
                <svg
                  viewBox="0 0 24 24"
                  width="15"
                  height="15"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2.2"
                  strokeLinecap="round"
                  strokeLinejoin="round"
                >
                  <path d="M20 6L9 17l-5-5" />
                </svg>
                {t("allclear")}
              </div>
            )}

            {filter === "all" && healthy.length > 0 && (
              <section>
                <div className="section-head">
                  <span className="section-title">{t("section.healthy")}</span>
                  <span className="section-count">{healthy.length}</span>
                </div>
                {healthy.map((e) => (
                  <Row
                    key={e.pid}
                    e={e}
                    expanded={expandedPid === e.pid}
                    {...rowProps}
                  />
                ))}
              </section>
            )}

            {filter === "whitelist" && (
              <section>
                <div className="section-head">
                  <span className="section-title">{t("section.starred")}</span>
                  <span className="section-count">{filtered.length}</span>
                </div>
                {filtered.map((e) => (
                  <Row
                    key={e.pid}
                    e={e}
                    expanded={expandedPid === e.pid}
                    {...rowProps}
                  />
                ))}
              </section>
            )}
          </>
        )}
      </main>

      <footer className="footer">
        {t("footer.status", { procs: entries.length, ports: totalPortCount })}
      </footer>

      {batchConfirm && (
        <div className="modal-backdrop" onClick={() => setBatchConfirm(null)}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="batch-modal-title"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={trapTab}
          >
            <div className="modal-title" id="batch-modal-title">
              {t("batch.title", { n: batchConfirm.length })}
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">{t("batch.signal")}</span>
                <span className="mono">
                  {os === "macos" ? t("batch.signal.macos") : t("batch.signal.windows")}
                </span>
              </div>
              <div className="modal-row modal-row-top">
                <span className="modal-label">{t("batch.procs")}</span>
                <div className="batch-list">
                  {batchConfirm.slice(0, 8).map((s) => (
                    <div key={s.pid} className="batch-item mono">
                      <span className="batch-pid">PID {s.pid}</span>
                      <span className="batch-label">{s.app_label}</span>
                      <span className="batch-ports muted">
                        {s.ports.map((p) => `:${p}`).join(" ")}
                      </span>
                    </div>
                  ))}
                  {batchConfirm.length > 8 && (
                    <div className="muted batch-more">
                      {t("batch.more", { n: batchConfirm.length - 8 })}
                    </div>
                  )}
                </div>
              </div>
              <div className="modal-row">
                <span className="muted">{t("batch.scope.note")}</span>
              </div>
            </div>
            <div className="modal-actions">
              {/* autoFocus 落在取消键：Enter 永远不应直接触发批量终止 */}
              <button
                className="btn-ghost"
                autoFocus
                onClick={() => setBatchConfirm(null)}
              >
                {t("batch.cancel")}
              </button>
              <button className="btn-danger-solid" onClick={doBatchKill}>
                {t("batch.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}

      {confirm && (
        <div className="modal-backdrop" onClick={() => setConfirm(null)}>
          <div
            className="modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby="confirm-modal-title"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={trapTab}
          >
            <div className="modal-title" id="confirm-modal-title">
              {confirm.force && os === "macos"
                ? t("confirm.title.force")
                : t("confirm.title.kill")}
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">{t("confirm.app")}</span>
                <span>{confirm.app_label}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">{t("confirm.cmd")}</span>
                <span className="mono">{confirm.command}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">{t("confirm.pid")}</span>
                <span className="mono">{confirm.pid}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">{t("confirm.ports")}</span>
                <span className="mono">
                  {confirm.ports.map((p) => `:${p}`).join("  ")}
                  {confirm.ports.length > 1 && (
                    <span className="muted">
                      {" "}
                      {t("confirm.portsRelease", { n: confirm.ports.length })}
                    </span>
                  )}
                </span>
              </div>
              <div className="modal-row">
                <span className="modal-label">{t("confirm.signal")}</span>
                <span className="mono">
                  {os === "windows"
                    ? t("confirm.signal.win")
                    : confirm.force
                      ? t("confirm.signal.kill")
                      : t("confirm.signal.term")}
                </span>
              </div>
            </div>
            <div className="modal-actions">
              {/* autoFocus 落在取消键：Enter 永远不应直接触发终止 */}
              <button className="btn-ghost" autoFocus onClick={() => setConfirm(null)}>
                {t("confirm.cancel")}
              </button>
              <button className="btn-danger-solid" onClick={doKill}>
                {confirm.force && os === "macos"
                  ? t("confirm.force")
                  : t("confirm.kill")}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

// ────────────────────────────────────────────────────────────────────────────

type RowProps = {
  e: ProcessEntry;
  expanded: boolean;
  os: Os;
  lang: Lang;
  killingPid: number | null;
  onAskKill: (e: ProcessEntry, force: boolean) => void;
  onToggleWhitelist: (e: ProcessEntry) => void;
  onOpenPort: (port: number) => void;
  onToggleExpand: (pid: number) => void;
};

function Row({
  e,
  expanded,
  os,
  lang,
  killingPid,
  onAskKill,
  onToggleWhitelist,
  onOpenPort,
  onToggleExpand,
}: RowProps) {
  const { t } = useI18n();

  // app_label 形如 "dev-server.js · node" —— 主名 + 次级说明
  const sep = e.app_label.indexOf(" · ");
  const name = sep >= 0 ? e.app_label.slice(0, sep) : e.app_label;
  const nameSub = sep >= 0 ? e.app_label.slice(sep + 3) : null;

  const known = describeEntry(e, lang);
  const desc = known ?? t(DESC_KEYS[e.app_category] ?? "desc.unknown");

  // 来源：谁启动的 / 谁在托管
  const exempt = !e.is_zombie_suspect
    ? e.zombie_reasons.filter((r) => EXEMPT_REASONS.has(r))
    : [];
  let provenance: string | null = null;
  if (exempt.includes("launchd_managed")) {
    provenance = t("story.managedBySystem");
  } else if (exempt.length > 0 && exempt[0] !== "installed_app") {
    provenance = t(`reason.${exempt[0]}` as I18nKey);
  } else if (e.launcher_label && e.launcher_label !== "?") {
    provenance =
      e.launcher_label === "launchd"
        ? t("story.launchedBySystem")
        : t("story.launchedBy", { app: e.launcher_label });
  }

  const primary = e.is_zombie_suspect ? primaryReason(e.zombie_reasons) : null;
  const shownPorts = e.ports.slice(0, 3);
  const morePorts = e.ports.length - shownPorts.length;
  const killing = killingPid === e.pid;

  return (
    <div className={`row-block ${expanded ? "open" : ""}`}>
      {/* 行整体可点（鼠标增强）；键盘路径走真实的折叠按钮 —— 避免 button 嵌套 */}
      <div
        className={`row ${e.is_zombie_suspect ? `row-suspect row-${e.confidence}` : ""}`}
        onClick={() => onToggleExpand(e.pid)}
      >
        <button
          className={`disclosure ${expanded ? "open" : ""}`}
          aria-expanded={expanded}
          aria-controls={`proc-detail-${e.pid}`}
          aria-label={t("row.expand.tip")}
          title={t("row.expand.tip")}
          onClick={(ev) => {
            ev.stopPropagation();
            onToggleExpand(e.pid);
          }}
        >
          <svg
            viewBox="0 0 24 24"
            width="12"
            height="12"
            fill="none"
            stroke="currentColor"
            strokeWidth="2.4"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            <path d="M9 6l6 6-6 6" />
          </svg>
        </button>

        <div className="row-main">
          <div className="row-title">
            <span className="row-name">{name}</span>
            {nameSub && <span className="row-name-sub">{nameSub}</span>}
            <span className="row-ports mono">
              {shownPorts.map((p) => (
                <button
                  key={p}
                  className="port-link"
                  onClick={(ev) => {
                    ev.stopPropagation();
                    onOpenPort(p);
                  }}
                  title={t("port.tip", { port: p })}
                >
                  :{p}
                </button>
              ))}
              {morePorts > 0 && (
                <span
                  className="port-more"
                  title={e.ports.map((p) => `:${p}`).join(" ")}
                >
                  +{morePorts}
                </span>
              )}
            </span>
          </div>
          <div className="row-desc">
            {e.is_zombie_suspect ? (
              <>
                <span className={`verdict verdict-${e.confidence}`}>
                  {t(`verdict.${e.confidence}` as I18nKey)}
                </span>
                {primary && (
                  <span className="desc-text">
                    {" · "}
                    {t(`story.${primary}` as I18nKey)}
                  </span>
                )}
                <span className="desc-text desc-dim">
                  {" · "}
                  {desc}
                </span>
              </>
            ) : (
              <>
                {e.is_whitelisted && (
                  <span className="desc-starred">★ {t("story.starred")} · </span>
                )}
                <span className="desc-text">{desc}</span>
                {provenance && (
                  <span className="desc-text desc-dim"> · {provenance}</span>
                )}
              </>
            )}
          </div>
        </div>

        <span className="row-uptime" title={formatDuration(e.elapsed_secs)}>
          {formatUptime(e.elapsed_secs, t)}
        </span>

        <div
          className={`row-actions ${e.is_zombie_suspect ? "always" : ""}`}
          onClick={(ev) => ev.stopPropagation()}
        >
          {os === "macos" ? (
            <>
              <button
                className={`btn-act btn-kill ${e.is_zombie_suspect ? "primary" : ""}`}
                onClick={() => onAskKill(e, false)}
                disabled={killing}
                title={t("kill.btn.tip")}
              >
                {t("kill.btn")}
              </button>
              <button
                className="btn-act btn-force"
                onClick={() => onAskKill(e, true)}
                disabled={killing}
                title={t("kill.force.tip")}
              >
                {t("kill.force.btn")}
              </button>
            </>
          ) : (
            <button
              className={`btn-act btn-kill ${e.is_zombie_suspect ? "primary" : ""}`}
              onClick={() => onAskKill(e, true)}
              disabled={killing}
              title={t("kill.terminate.tip")}
            >
              {t("kill.terminate.btn")}
            </button>
          )}
          <button
            className={`btn-act btn-star ${e.is_whitelisted ? "active" : ""}`}
            onClick={() => onToggleWhitelist(e)}
            title={e.is_whitelisted ? t("star.remove.tip") : t("star.add.tip")}
          >
            {e.is_whitelisted ? "★" : "☆"}
          </button>
        </div>
      </div>

      {expanded && <Detail e={e} os={os} id={`proc-detail-${e.pid}`} />}
    </div>
  );
}

function Detail({ e, os, id }: { e: ProcessEntry; os: Os; id: string }) {
  const { t } = useI18n();

  // 链末节点用主名（app_label 可能带 " · node" 次级说明，链里不需要）
  const sepIdx = e.app_label.indexOf(" · ");
  const selfName = sepIdx >= 0 ? e.app_label.slice(0, sepIdx) : e.app_label;

  const catKey = (
    ["installed-app", "system", "dev-script", "user-binary"].includes(e.app_category)
      ? `cat.${e.app_category}`
      : "cat.unknown"
  ) as I18nKey;

  const exempt = !e.is_zombie_suspect
    ? e.zombie_reasons.filter((r) => EXEMPT_REASONS.has(r))
    : [];

  // 启动链：根（顶端 App / 系统）在前，依次到直接父进程，最后是进程本身
  const chainTopDown = [...e.parent_chain].reverse();

  return (
    <div className="detail" id={id}>
      <div className="detail-grid">
        <span className="detail-label">{t("detail.command")}</span>
        <span className="detail-value mono selectable">{e.full_command || e.command}</span>

        <span className="detail-label">{t("detail.path")}</span>
        <span className="detail-value mono selectable">{e.exe_path || "—"}</span>

        <span className="detail-label">{t("detail.ports")}</span>
        <span className="detail-value mono">
          {e.ports.map((p) => `:${p}`).join("  ")}
        </span>

        <span className="detail-label">{t("detail.pid")}</span>
        <span className="detail-value mono">
          {e.pid}
          <span className="detail-sep">·</span>
          <span className="detail-dim">
            {t("detail.parent")} {e.ppid}
          </span>
          {os === "macos" && e.ppid === 1 && (
            <span className="detail-note">{t("detail.parent.launchdNote")}</span>
          )}
        </span>

        <span className="detail-label">{t("detail.category")}</span>
        <span className="detail-value">{t(catKey)}</span>

        <span className="detail-label">{t("detail.resources")}</span>
        <span className="detail-value mono">
          {t("detail.resources.value", {
            cpu: e.cpu_percent.toFixed(1),
            mem: e.mem_mb.toFixed(1),
            uptime: formatDuration(e.elapsed_secs),
          })}
        </span>

        <span className="detail-label">{t("detail.chain")}</span>
        <span className="detail-value">
          {chainTopDown.length === 0 ? (
            <span className="detail-dim">{t("detail.chain.empty")}</span>
          ) : (
            <span className="chain">
              {chainTopDown.map((p, i) => (
                <span className="chain-node" key={`${p.pid}-${i}`}>
                  {i > 0 && <span className="chain-arrow">›</span>}
                  {p.label}
                </span>
              ))}
              <span className="chain-arrow">›</span>
              <span className="chain-node chain-self">{selfName}</span>
            </span>
          )}
        </span>
      </div>

      {e.is_zombie_suspect && e.zombie_reasons.length > 0 && (
        <div className="evidence">
          <div className="evidence-title evidence-title-danger">
            {t("detail.evidence")}
          </div>
          {e.zombie_reasons.map((r) => (
            <div className="evidence-item" key={r}>
              <span className="evidence-name">{t(`reason.${r}` as I18nKey)}</span>
              <span className="evidence-text">{t(`reasonTip.${r}` as I18nKey)}</span>
            </div>
          ))}
        </div>
      )}

      {exempt.length > 0 && (
        <div className="evidence">
          <div className="evidence-title">{t("detail.whyNot")}</div>
          {exempt.map((r) => (
            <div className="evidence-item" key={r}>
              <span className="evidence-name evidence-name-ok">
                ✓ {t(`reason.${r}` as I18nKey)}
              </span>
              <span className="evidence-text">{t(`reasonTip.${r}` as I18nKey)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

export default App;
