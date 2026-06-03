import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n, type I18nKey } from "./i18n";
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

const CATEGORY_META: Record<string, { color: string; bg: string; border: string }> = {
  "installed-app": {
    color: "#10b981",
    bg: "rgba(16, 185, 129, 0.12)",
    border: "rgba(16, 185, 129, 0.32)",
  },
  system: {
    color: "#60a5fa",
    bg: "rgba(96, 165, 250, 0.12)",
    border: "rgba(96, 165, 250, 0.3)",
  },
  "dev-script": {
    color: "#f59e0b",
    bg: "rgba(245, 158, 11, 0.12)",
    border: "rgba(245, 158, 11, 0.3)",
  },
  "user-binary": {
    color: "#a78bfa",
    bg: "rgba(167, 139, 250, 0.13)",
    border: "rgba(167, 139, 250, 0.3)",
  },
  unknown: {
    color: "#6b7280",
    bg: "rgba(107, 114, 128, 0.15)",
    border: "rgba(107, 114, 128, 0.3)",
  },
};

/** 置信度徽章配色：确认红 / 很可能琥珀 / 存疑蓝灰 */
const CONFIDENCE_META: Record<
  Exclude<Confidence, "none">,
  { color: string; bg: string; border: string }
> = {
  confirmed: {
    color: "#ef4444",
    bg: "rgba(239, 68, 68, 0.14)",
    border: "rgba(239, 68, 68, 0.4)",
  },
  likely: {
    color: "#f59e0b",
    bg: "rgba(245, 158, 11, 0.13)",
    border: "rgba(245, 158, 11, 0.38)",
  },
  possible: {
    color: "#94a3b8",
    bg: "rgba(148, 163, 184, 0.13)",
    border: "rgba(148, 163, 184, 0.35)",
  },
};

/** 豁免类 reason code —— 非嫌疑行上以「为什么没被标记」的形式展示 */
const EXEMPT_REASONS = new Set([
  "launchd_managed",
  "brew_service_path",
  "installed_app",
  "pm2_managed",
]);

/** 一键清扫覆盖的置信层级（Possible 永不入清扫） */
const SWEEPABLE: ReadonlySet<Confidence> = new Set(["confirmed", "likely"]);

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
  const [lastScan, setLastScan] = useState<Date | null>(null);
  const inFlight = useRef(false);

  useEffect(() => {
    invoke<Os>("get_platform")
      .then(setOs)
      .catch(() => {});
  }, []);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const data = await invoke<ProcessEntry[]>("scan_ports");
      setEntries(data);
      setLastScan(new Date());
      // 托盘只计入会被清扫的层级（Confirmed + Likely），避免宽限期内的闪烁
      const suspectCount = data.filter(
        (e) => e.is_zombie_suspect && SWEEPABLE.has(e.confidence),
      ).length;
      const totalPorts = data.reduce((sum, e) => sum + e.ports.length, 0);
      await invoke("update_tray_title", {
        count: totalPorts,
        suspectCount,
      });
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    refresh();
    const id = setInterval(refresh, 2000);
    return () => clearInterval(id);
  }, [refresh]);

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
      await refresh();
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
    inFlight.current = false; // 防止与 2s 自动刷新撞车导致这次 refresh 被静默丢掉
    await refresh();
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

  return (
    <div className="app">
      <header className="header">
        <div className="title-row">
          <div className="brand">
            <svg
              className="brand-icon"
              viewBox="0 0 24 24"
              width="22"
              height="22"
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
          <div className="pills">
            <span className="pill">
              <span className="pill-num">{entries.length}</span>
              <span className="pill-label">{t("pills.processes")}</span>
            </span>
            <span className="pill">
              <span className="pill-num">{totalPortCount}</span>
              <span className="pill-label">{t("pills.ports")}</span>
            </span>
            <span className={`pill ${suspectCount > 0 ? "danger" : ""}`}>
              <span className="pill-num">{suspectCount}</span>
              <span className="pill-label">{t("pills.suspects")}</span>
            </span>
            <span className="pill mute">
              <span className="pill-num">{whitelistCount}</span>
              <span className="pill-label">{t("pills.whitelisted")}</span>
            </span>
          </div>
          <div className="meta">
            {lastScan && <span className="last-scan">{t("header.autoRefresh")}</span>}
            <button
              className="btn-ghost lang-toggle"
              onClick={() => setLang(lang === "zh" ? "en" : "zh")}
              title={lang === "zh" ? "Switch to English" : "切换为中文"}
            >
              {lang === "zh" ? "EN" : "中"}
            </button>
          </div>
        </div>

        <div className="toolbar">
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
            </button>
            <button
              className={filter === "suspect" ? "active" : ""}
              onClick={() => setFilter("suspect")}
            >
              {t("filter.suspect")}
            </button>
            <button
              className={filter === "whitelist" ? "active" : ""}
              onClick={() => setFilter("whitelist")}
            >
              {t("filter.whitelist")}
            </button>
          </div>
          <button
            className="btn-cleanup"
            disabled={sweepables.length === 0 || sweeping}
            onClick={askKillAllSuspects}
            title={t("sweep.title")}
          >
            {sweeping
              ? t("sweep.sweeping")
              : `${t("sweep.button")} ${sweepables.length > 0 ? `(${sweepables.length})` : ""}`}
          </button>
          <button
            className="btn-ghost btn-refresh"
            onClick={refresh}
            title={t("refresh.title")}
            aria-label={t("refresh.aria")}
          >
            ↻
          </button>
        </div>
      </header>

      {error && (
        <div className="error" onClick={() => setError(null)}>
          {error} {t("error.clickToClose")}
        </div>
      )}

      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th style={{ width: 96 }} title={t("th.ports.tip")}>
                {t("th.ports")}
              </th>
              <th style={{ width: 88 }} title={t("th.type.tip")}>
                {t("th.type")}
              </th>
              <th style={{ width: 360 }} title={t("th.app.tip")}>
                {t("th.app")}
              </th>
              <th className="th-path" title={t("th.path.tip")}>
                {t("th.path")}
              </th>
              <th style={{ width: 150 }} title={t("th.launcher.tip")}>
                {t("th.launcher")}
              </th>
              <th
                style={{ width: 150 }}
                title={os === "macos" ? t("th.pid.tip.macos") : t("th.pid.tip.windows")}
              >
                {t("th.pid")}
              </th>
              <th style={{ width: 100 }} title={t("th.elapsed.tip")}>
                {t("th.elapsed")}
              </th>
              <th style={{ width: 78 }} title={t("th.cpu.tip")}>
                {t("th.cpu")}
              </th>
              <th style={{ width: 92 }} title={t("th.mem.tip")}>
                {t("th.mem")}
              </th>
              <th
                style={{ width: 198 }}
                title={
                  os === "macos"
                    ? t("th.actions.tip.macos")
                    : t("th.actions.tip.windows")
                }
              >
                {t("th.actions")}
              </th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((e) => {
              const meta = CATEGORY_META[e.app_category] || CATEGORY_META.unknown;
              const catLabelKey = (
                CATEGORY_META[e.app_category] ? `cat.${e.app_category}` : "cat.unknown"
              ) as I18nKey;
              const confMeta =
                e.confidence !== "none" ? CONFIDENCE_META[e.confidence] : null;
              const exemptReasons = !e.is_zombie_suspect
                ? e.zombie_reasons.filter((r) => EXEMPT_REASONS.has(r))
                : [];
              const showCmd =
                e.full_command && e.full_command !== (e.exe_path || e.command);
              return (
                <tr key={e.pid} className={e.is_zombie_suspect ? "suspect" : ""}>
                  <td className="ports-cell">
                    <div className="ports-list">
                      {e.ports.map((p) => (
                        <button
                          key={p}
                          className="port-link"
                          onClick={() => handleOpen(p)}
                          title={t("port.tip", { port: p })}
                        >
                          :{p}
                        </button>
                      ))}
                    </div>
                  </td>
                  <td className="cat-cell">
                    <span
                      className="cat-badge"
                      style={{
                        color: meta.color,
                        background: meta.bg,
                        borderColor: meta.border,
                      }}
                    >
                      {t(catLabelKey)}
                    </span>
                  </td>
                  <td className="app-cell">
                    <AppLabel label={e.app_label} whitelisted={e.is_whitelisted} />
                    {e.is_zombie_suspect && (
                      <div className="suspect-tags">
                        {confMeta && (
                          <span
                            className="chip"
                            style={{
                              color: confMeta.color,
                              background: confMeta.bg,
                              borderColor: confMeta.border,
                            }}
                          >
                            {t(`confidence.${e.confidence}` as I18nKey)}
                          </span>
                        )}
                        {e.zombie_reasons.map((r) => (
                          <span
                            key={r}
                            className="chip chip-sus"
                            title={t(`reasonTip.${r}` as I18nKey)}
                          >
                            {t(`reason.${r}` as I18nKey)}
                          </span>
                        ))}
                      </div>
                    )}
                    {exemptReasons.length > 0 && (
                      <div className="suspect-tags">
                        {exemptReasons.map((r) => (
                          <span
                            key={r}
                            className="chip chip-exempt"
                            title={
                              t("exempt.tip") + t(`reasonTip.${r}` as I18nKey)
                            }
                          >
                            ✓ {t(`reason.${r}` as I18nKey)}
                          </span>
                        ))}
                      </div>
                    )}
                  </td>
                  <td className="path-cell">
                    <div className="path-line mono" title={e.exe_path || e.command}>
                      {e.exe_path || e.command}
                    </div>
                    {showCmd && (
                      <div
                        className="path-args mono"
                        title={t("cmd.full.tip", { cmd: e.full_command })}
                      >
                        {t("args.label")}:{" "}
                        {e.full_command.slice((e.exe_path || e.command).length).trim()}
                      </div>
                    )}
                  </td>
                  <td>
                    <LauncherChain entry={e} />
                  </td>
                  <td className="pid-cell">
                    <div className="pid-line" title={t("pid.self.tip")}>
                      <span className="pid-label">{t("pid.self")}</span>
                      <span className="pid-num mono">{e.pid}</span>
                    </div>
                    <div
                      className="pid-line"
                      title={
                        os === "macos" && e.ppid === 1
                          ? t("pid.parent.tip.launchd")
                          : t("pid.parent.tip.normal")
                      }
                    >
                      <span className="pid-label">{t("pid.parent")}</span>
                      <span className="pid-num mono">
                        {os === "macos" && e.ppid === 1 ? "1 · launchd" : e.ppid}
                      </span>
                    </div>
                  </td>
                  <td className="time-cell mono">{formatDuration(e.elapsed_secs)}</td>
                  <td
                    className="metric-cell mono"
                    title={t("cpu.tip", { v: e.cpu_percent.toFixed(1) })}
                  >
                    {e.cpu_percent.toFixed(1)}%
                  </td>
                  <td
                    className="metric-cell mono"
                    title={t("mem.tip", { v: e.mem_mb.toFixed(1) })}
                  >
                    {e.mem_mb.toFixed(1)} MB
                  </td>
                  <td className="actions">
                    {os === "macos" ? (
                      <>
                        <button
                          className="btn-kill"
                          onClick={() => askKill(e, false)}
                          disabled={killingPid === e.pid}
                        >
                          {t("kill.btn")}
                        </button>
                        <button
                          className="btn-force"
                          onClick={() => askKill(e, true)}
                          disabled={killingPid === e.pid}
                          title={t("kill.force.tip")}
                        >
                          {t("kill.force.btn")}
                        </button>
                      </>
                    ) : (
                      <button
                        className="btn-force"
                        onClick={() => askKill(e, true)}
                        disabled={killingPid === e.pid}
                        title={t("kill.terminate.tip")}
                      >
                        {t("kill.terminate.btn")}
                      </button>
                    )}
                    <button
                      className={`btn-star ${e.is_whitelisted ? "active" : ""}`}
                      onClick={() => handleToggleWhitelist(e)}
                      title={e.is_whitelisted ? t("star.remove.tip") : t("star.add.tip")}
                    >
                      {e.is_whitelisted ? "★" : "☆"}
                    </button>
                  </td>
                </tr>
              );
            })}
            {filtered.length === 0 && (
              <tr>
                <td colSpan={10} className="empty">
                  {entries.length === 0 ? t("empty.none") : t("empty.noMatch")}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {batchConfirm && (
        <div className="modal-backdrop" onClick={() => setBatchConfirm(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">
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
              <button className="btn-ghost" onClick={() => setBatchConfirm(null)}>
                {t("batch.cancel")}
              </button>
              <button className="btn-kill solid" onClick={doBatchKill}>
                {t("batch.confirm")}
              </button>
            </div>
          </div>
        </div>
      )}

      {confirm && (
        <div className="modal-backdrop" onClick={() => setConfirm(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">
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
              <button className="btn-ghost" onClick={() => setConfirm(null)}>
                {t("confirm.cancel")}
              </button>
              <button
                className={confirm.force ? "btn-force solid" : "btn-kill solid"}
                onClick={doKill}
              >
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

function AppLabel({ label, whitelisted }: { label: string; whitelisted: boolean }) {
  const { t } = useI18n();
  const idx = label.indexOf(" · ");
  const main = idx >= 0 ? label.slice(0, idx) : label;
  const sub = idx >= 0 ? label.slice(idx + 3) : null;
  return (
    <div className="app-stack">
      <div className="app-line-1">
        <span className="app-name-main" title={label}>
          {main}
        </span>
        {whitelisted && <span className="chip chip-wl">{t("whitelist.chip")}</span>}
      </div>
      {sub && (
        <div className="app-name-sub" title={sub}>
          {sub}
        </div>
      )}
    </div>
  );
}

function LauncherChain({ entry }: { entry: ProcessEntry }) {
  const chain = entry.parent_chain;
  if (chain.length === 0) {
    return <span className="launcher-empty">—</span>;
  }
  const top = chain[chain.length - 1];
  const topMeta = CATEGORY_META[top.category] || CATEGORY_META.unknown;
  // 倒序：从靠近顶端 App 的中间过程，一层层缩进到直接父进程
  const intermediate = chain.slice(0, -1).reverse();

  const tooltip = chain
    .map((p: ParentRef) => `${p.label} (PID ${p.pid})`)
    .reverse()
    .join("  →  ");

  return (
    <div className="launcher" title={tooltip}>
      <div
        className="launcher-top"
        style={{ color: topMeta.color, borderColor: topMeta.border }}
      >
        {top.label}
      </div>
      {intermediate.map((p: ParentRef, i: number) => {
        const m = CATEGORY_META[p.category] || CATEGORY_META.unknown;
        return (
          <div
            key={`${p.pid}-${i}`}
            className="launcher-tree-node"
            style={{ paddingLeft: `${(i + 1) * 14}px`, color: m.color }}
          >
            <span className="launcher-tree-branch">└</span>
            <span className="launcher-tree-label">{p.label}</span>
          </div>
        );
      })}
    </div>
  );
}

export default App;
