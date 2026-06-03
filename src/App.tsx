import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import "./App.css";

type ParentRef = {
  pid: number;
  label: string;
  category: string;
  exe_path: string;
};

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
  elapsed: string;
  cpu_percent: number;
  mem_mb: number;
  state: string;
  is_zombie_suspect: boolean;
  zombie_reasons: string[];
  is_whitelisted: boolean;
};

type Filter = "all" | "suspect" | "whitelist";

type ConfirmState = {
  pid: number;
  ports: number[];
  command: string;
  app_label: string;
  force: boolean;
} | null;

const CATEGORY_META: Record<
  string,
  { label: string; color: string; bg: string; border: string }
> = {
  "installed-app": {
    label: "APP",
    color: "#10b981",
    bg: "rgba(16, 185, 129, 0.12)",
    border: "rgba(16, 185, 129, 0.32)",
  },
  system: {
    label: "系统",
    color: "#60a5fa",
    bg: "rgba(96, 165, 250, 0.12)",
    border: "rgba(96, 165, 250, 0.3)",
  },
  "dev-script": {
    label: "脚本",
    color: "#f59e0b",
    bg: "rgba(245, 158, 11, 0.12)",
    border: "rgba(245, 158, 11, 0.3)",
  },
  "user-binary": {
    label: "CLI",
    color: "#a78bfa",
    bg: "rgba(167, 139, 250, 0.13)",
    border: "rgba(167, 139, 250, 0.3)",
  },
  unknown: {
    label: "?",
    color: "#6b7280",
    bg: "rgba(107, 114, 128, 0.15)",
    border: "rgba(107, 114, 128, 0.3)",
  },
};

function App() {
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

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const data = await invoke<ProcessEntry[]>("scan_ports");
      setEntries(data);
      setLastScan(new Date());
      const suspectCount = data.filter((e) => e.is_zombie_suspect).length;
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
    });
  };

  const doKill = async () => {
    if (!confirm) return;
    const { pid, force } = confirm;
    setKillingPid(pid);
    setConfirm(null);
    try {
      await invoke("kill_process", { pid, force });
      await new Promise((r) => setTimeout(r, 250));
      await refresh();
    } catch (err) {
      setError(`Kill 失败: ${String(err)}`);
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
      setError(`打开浏览器失败: ${String(err)}`);
    }
  };

  const askKillAllSuspects = () => {
    const suspects = entries.filter((e) => e.is_zombie_suspect);
    if (suspects.length === 0) return;
    setBatchConfirm(suspects);
  };

  const doBatchKill = async () => {
    const suspects = batchConfirm;
    if (!suspects || suspects.length === 0) return;
    setBatchConfirm(null);
    setSweeping(true);
    const failures: { pid: number; label: string; err: string }[] = [];
    for (const s of suspects) {
      try {
        await invoke("kill_process", { pid: s.pid, force: false });
      } catch (err) {
        failures.push({ pid: s.pid, label: s.app_label, err: String(err) });
      }
    }
    await new Promise((r) => setTimeout(r, 700));
    inFlight.current = false; // 防止与 2s 自动刷新撞车导致这次 refresh 被静默丢掉
    await refresh();
    setSweeping(false);
    if (failures.length > 0) {
      setError(
        `${failures.length}/${suspects.length} 个进程 kill 失败：` +
          failures
            .map((f) => `PID ${f.pid} ${f.label} (${f.err})`)
            .join("；"),
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
              <span className="pill-label">进程</span>
            </span>
            <span className="pill">
              <span className="pill-num">{totalPortCount}</span>
              <span className="pill-label">端口</span>
            </span>
            <span className={`pill ${suspectCount > 0 ? "danger" : ""}`}>
              <span className="pill-num">{suspectCount}</span>
              <span className="pill-label">疑似僵尸</span>
            </span>
            <span className="pill mute">
              <span className="pill-num">{whitelistCount}</span>
              <span className="pill-label">已收藏</span>
            </span>
          </div>
          <div className="meta">
            {lastScan && <span className="last-scan">· 每 2s 自动刷新</span>}
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
              placeholder="搜索 端口 / PID / App / 启动方"
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
              全部
            </button>
            <button
              className={filter === "suspect" ? "active" : ""}
              onClick={() => setFilter("suspect")}
            >
              仅疑似僵尸
            </button>
            <button
              className={filter === "whitelist" ? "active" : ""}
              onClick={() => setFilter("whitelist")}
            >
              已收藏
            </button>
          </div>
          <button
            className="btn-cleanup"
            disabled={suspectCount === 0 || sweeping}
            onClick={askKillAllSuspects}
            title="批量 kill 所有疑似僵尸进程"
          >
            {sweeping ? "清扫中…" : `一键清扫 ${suspectCount > 0 ? `(${suspectCount})` : ""}`}
          </button>
          <button
            className="btn-ghost btn-refresh"
            onClick={refresh}
            title="立即刷新（自动每 2 秒会刷一次）"
            aria-label="立即刷新"
          >
            ↻
          </button>
        </div>
      </header>

      {error && (
        <div className="error" onClick={() => setError(null)}>
          {error} · 点击关闭
        </div>
      )}

      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th
                style={{ width: 96 }}
                title="进程正在 TCP LISTEN 的本地端口。点击端口 chip 会在浏览器打开 http://localhost:PORT"
              >
                端口
              </th>
              <th
                style={{ width: 88 }}
                title="进程分类（脚本 / APP / 系统 / CLI）"
              >
                类型
              </th>
              <th
                style={{ width: 360 }}
                title="App 名称（由 Rust 端的 identify_app 算出的人类可读标签）"
              >
                App
              </th>
              <th
                className="th-path"
                title="可执行文件的完整路径。鼠标悬停在路径上可查看完整命令行（含所有参数）"
              >
                路径
              </th>
              <th
                style={{ width: 150 }}
                title="沿父进程链向上找到的第一个用户可见 App；下方小字是中间的脚本/进程"
              >
                启动者
              </th>
              <th
                style={{ width: 150 }}
                title="本进程 PID / 父进程 PID。父进程是 launchd (PID 1) 意味着原启动者已退出 —— 这是「孤儿」信号。"
              >
                PID
              </th>
              <th
                style={{ width: 100 }}
                title="进程已运行时长（HH:MM:SS，超过 1 天显示为 dd-HH:MM:SS）"
              >
                已运行
              </th>
              <th
                style={{ width: 78 }}
                title="进程占用 CPU 的百分比（瞬时采样）"
              >
                CPU
              </th>
              <th
                style={{ width: 92 }}
                title="进程常驻物理内存（RSS）"
              >
                内存
              </th>
              <th
                style={{ width: 198 }}
                title="Kill = SIGTERM 优雅终止；强杀 = SIGKILL 强制终止；★ 加入收藏后不再被判为僵尸"
              >
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            {filtered.map((e) => {
              const meta =
                CATEGORY_META[e.app_category] || CATEGORY_META.unknown;
              const showCmd =
                e.full_command && e.full_command !== (e.exe_path || e.command);
              return (
                <tr
                  key={e.pid}
                  className={e.is_zombie_suspect ? "suspect" : ""}
                >
                  <td className="ports-cell">
                    <div className="ports-list">
                      {e.ports.map((p) => (
                        <button
                          key={p}
                          className="port-link"
                          onClick={() => handleOpen(p)}
                          title={`在浏览器打开 http://localhost:${p}`}
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
                      {meta.label}
                    </span>
                  </td>
                  <td className="app-cell">
                    <AppLabel
                      label={e.app_label}
                      whitelisted={e.is_whitelisted}
                    />
                    {e.is_zombie_suspect && (
                      <div className="suspect-tags">
                        {e.zombie_reasons.map((r) => (
                          <span key={r} className="chip chip-sus">
                            {r}
                          </span>
                        ))}
                      </div>
                    )}
                  </td>
                  <td className="path-cell">
                    <div
                      className="path-line mono"
                      title={e.exe_path || e.command}
                    >
                      {e.exe_path || e.command}
                    </div>
                    {showCmd && (
                      <div
                        className="path-args mono"
                        title={`完整命令：${e.full_command}`}
                      >
                        参数: {e.full_command.slice((e.exe_path || e.command).length).trim()}
                      </div>
                    )}
                  </td>
                  <td>
                    <LauncherChain entry={e} />
                  </td>
                  <td className="pid-cell">
                    <div
                      className="pid-line"
                      title="本进程 PID（lsof 检测到正在监听端口的进程）"
                    >
                      <span className="pid-label">本进程</span>
                      <span className="pid-num mono">{e.pid}</span>
                    </div>
                    <div
                      className="pid-line"
                      title={
                        e.ppid === 1
                          ? "父进程 PID 1 = launchd —— macOS 的进程主控（开机后第一个启动，所有进程的最终祖先）。父进程为 launchd 通常意味着原启动者（终端 / IDE）已退出，本进程被「收养」为孤儿。"
                          : "父进程 PID（启动本进程的进程）"
                      }
                    >
                      <span className="pid-label">父进程</span>
                      <span className="pid-num mono">
                        {e.ppid === 1 ? "1 · launchd" : e.ppid}
                      </span>
                    </div>
                  </td>
                  <td className="time-cell mono">{e.elapsed}</td>
                  <td className="metric-cell mono" title={`CPU 占用 ${e.cpu_percent.toFixed(1)}%`}>
                    {e.cpu_percent.toFixed(1)}%
                  </td>
                  <td className="metric-cell mono" title={`常驻物理内存 ${e.mem_mb.toFixed(1)} MB`}>
                    {e.mem_mb.toFixed(1)} MB
                  </td>
                  <td className="actions">
                    <button
                      className="btn-kill"
                      onClick={() => askKill(e, false)}
                      disabled={killingPid === e.pid}
                    >
                      Kill
                    </button>
                    <button
                      className="btn-force"
                      onClick={() => askKill(e, true)}
                      disabled={killingPid === e.pid}
                      title="kill -9 强制"
                    >
                      强杀
                    </button>
                    <button
                      className={`btn-star ${e.is_whitelisted ? "active" : ""}`}
                      onClick={() => handleToggleWhitelist(e)}
                      title={
                        e.is_whitelisted
                          ? "从收藏移除"
                          : "加入收藏（不再被标记为僵尸）"
                      }
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
                  {entries.length === 0
                    ? "没有发现任何监听端口"
                    : "没有匹配项"}
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
              一键清扫 {batchConfirm.length} 个疑似僵尸进程
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">信号</span>
                <span className="mono">SIGTERM (-15) 优雅终止</span>
              </div>
              <div className="modal-row modal-row-top">
                <span className="modal-label">进程</span>
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
                      … 还有 {batchConfirm.length - 8} 个
                    </div>
                  )}
                </div>
              </div>
            </div>
            <div className="modal-actions">
              <button className="btn-ghost" onClick={() => setBatchConfirm(null)}>
                取消
              </button>
              <button className="btn-kill solid" onClick={doBatchKill}>
                全部终止
              </button>
            </div>
          </div>
        </div>
      )}

      {confirm && (
        <div className="modal-backdrop" onClick={() => setConfirm(null)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title">
              {confirm.force ? "强制杀死进程" : "终止进程"}
            </div>
            <div className="modal-body">
              <div className="modal-row">
                <span className="modal-label">应用</span>
                <span>{confirm.app_label}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">命令</span>
                <span className="mono">{confirm.command}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">PID</span>
                <span className="mono">{confirm.pid}</span>
              </div>
              <div className="modal-row">
                <span className="modal-label">端口</span>
                <span className="mono">
                  {confirm.ports.map((p) => `:${p}`).join("  ")}
                  {confirm.ports.length > 1 && (
                    <span className="muted">
                      {" "}
                      · 杀掉进程会同时释放这 {confirm.ports.length} 个端口
                    </span>
                  )}
                </span>
              </div>
              <div className="modal-row">
                <span className="modal-label">信号</span>
                <span className="mono">
                  {confirm.force
                    ? "SIGKILL (-9) 不可被忽略"
                    : "SIGTERM (-15) 优雅终止"}
                </span>
              </div>
            </div>
            <div className="modal-actions">
              <button className="btn-ghost" onClick={() => setConfirm(null)}>
                取消
              </button>
              <button
                className={confirm.force ? "btn-force solid" : "btn-kill solid"}
                onClick={doKill}
              >
                {confirm.force ? "强制杀死" : "终止"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}

function AppLabel({
  label,
  whitelisted,
}: {
  label: string;
  whitelisted: boolean;
}) {
  const idx = label.indexOf(" · ");
  const main = idx >= 0 ? label.slice(0, idx) : label;
  const sub = idx >= 0 ? label.slice(idx + 3) : null;
  return (
    <div className="app-stack">
      <div className="app-line-1">
        <span className="app-name-main" title={label}>
          {main}
        </span>
        {whitelisted && <span className="chip chip-wl">★ 收藏</span>}
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
