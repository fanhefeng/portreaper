import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n } from "./i18n";
import {
  ACTION_TIMEOUT_MS,
  isSameProcess,
  killErrorCode,
  legacyWhitelistKey,
  localizeActionError,
  localizeKillError,
  localizeScanError,
  sweepableEntries,
  withTimeout,
  type Filter,
  type Os,
  type ProcessEntry,
  type Translator,
} from "./model";
import { useScan } from "./useScan";
import { Section } from "./components/Section";
import { BatchConfirmModal } from "./components/BatchConfirmModal";
import { KillConfirmModal, type KillConfirm } from "./components/KillConfirmModal";
import "./App.css";

/** 操作错误存「渲染函数」而非成品文案：横幅挂着时切换语言要跟着重译 ——
 *  与 scanError（存原始码、渲染时翻译）的语义对称（评审发现）。 */
type ActionError = { render: (t: Translator) => string };

// raw 是 invoke 的 reject 原值（结构化 KillError 或超时 sentinel），刻意不转字符串：
// 本地化要到渲染时才做（语言可切），而结构一旦压平就再也分派不出语义分支。
type BatchFailure = { pid: number; label: string; raw: unknown };

/** 变更类 invoke（kill / 白名单）统一包超时：后端挂起时各调用方的 finally
 *  才能执行，sweeping/killingPid 不会永久卡死按钮（评审发现；scan 侧的同类
 *  防护是 useScan.runScan 里的 withTimeout，缘由见 model.ts ACTION_TIMEOUT_MS）。 */
function invokeAction(cmd: string, args: Record<string, unknown>): Promise<void> {
  return withTimeout(invoke<void>(cmd, args), ACTION_TIMEOUT_MS, "ERR_ACTION_TIMEOUT");
}

/** 终止后的存活确认窗口。SIGTERM 是异步的，收到它的 dev server 关 HTTP
 *  keep-alive、停 esbuild service 常要 0.3–2s，过早宣布「还活着」会变成一片误报；
 *  超过这个上限还在，才是真的没死。 */
const CONFIRM_KILL_DEADLINE_MS = 2500;
const CONFIRM_KILL_POLL_MS = 400;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * 终止之后**没有死掉**的那个目标。
 *
 * 在此之前，三层（引擎 / IPC / UI）都把「信号已投递」当成「进程已终止」：
 * `libc::kill` 返回 0 就是成功。对忽略 SIGTERM 的进程，用户点完终止只会看到
 * 那一行安静地留在列表里 —— 没有红条，也没有任何解释。
 *
 * 自带重开确认框所需的全部信息（label/command/ports），**不依赖那一行还在
 * entries 里**：横幅挂着的这段时间里列表一直在刷新，去 `entries.find` 取目标
 * 会在进程刚好换了形态时静默失败 —— 一个点下去什么都不发生的按钮。
 */
type Survivor = {
  pid: number;
  label: string;
  startUnix: number;
  command: string;
  ports: number[];
};

function App() {
  const { t, lang, setLang } = useI18n();
  // os 在 get_platform 落定前为 null：终止按钮的布局是平台分叉的（macOS 双按钮 /
  // Windows 单按钮），落定前不渲染，避免 Windows 上首帧闪现 SIGTERM 双按钮（评审发现）。
  // get_platform 失败时回退 macOS（保守：双按钮语义在 Windows 后端也安全 —— force 被忽略）。
  const [os, setOs] = useState<Os | null>(null);
  // 扫描轮询（entries / scanError / freshScan）整体在 useScan 里：并发语义
  //（inFlight 复用、freshScan 等在途）在那儿有直接单测。
  const { entries, scanError, hasScanned, lastScanOk, clearScanError, freshScan } = useScan();
  const [filter, setFilter] = useState<Filter>("all");
  const [search, setSearch] = useState("");
  // 错误分两路（评审发现）：扫描错误由下一次成功轮询自动清除（自愈语义，useScan 负责）；
  // 操作错误（kill / 清扫 / 收藏 / 打开浏览器）只能用户点击关闭 ——
  // 否则 2s 轮询会在用户看清之前把失败原因静默冲掉。
  const [actionError, setActionError] = useState<ActionError | null>(null);
  const error = actionError
    ? actionError.render(t)
    : scanError
      ? localizeScanError(scanError, t)
      : null;
  // 横幅关闭集中一处：onClick 与键盘（Enter/空格）共用，防两路清除逻辑漂移
  const dismissError = useCallback(() => {
    setActionError(null);
    clearScanError();
  }, [clearScanError]);
  // 按 PID 成集，而不是单个「当前正在杀谁」（评审发现）：kill 是可并发发起的，
  // 单值状态会被后发起的那个覆盖，先发起的行随即恢复可点 —— 其请求明明还在飞。
  const [killingPids, setKillingPids] = useState<ReadonlySet<number>>(() => new Set());
  // 与 actionError 并列的第三路提示：它不是「操作失败」（信号确实送到了），
  // 而是「操作没有达成目的」，且带一个可操作的出口（强制终止）。用中性色，
  // 别混进红色错误横幅 —— 那会让「杀不掉」和「调用出错」看起来是一回事。
  const [survivor, setSurvivor] = useState<Survivor | null>(null);
  const [confirm, setConfirm] = useState<KillConfirm | null>(null);
  const [batchConfirm, setBatchConfirm] = useState<ProcessEntry[] | null>(null);
  const [sweeping, setSweeping] = useState(false);
  const [expandedPid, setExpandedPid] = useState<number | null>(null);
  // 弹窗关闭后焦点还给触发按钮（a11y：键盘用户不丢上下文）
  const modalTrigger = useRef<HTMLElement | null>(null);

  useEffect(() => {
    invoke<Os>("get_platform")
      .then(setOs)
      .catch(() => setOs("macos"));
  }, []);

  // 目标终于自己退了（或被别的方式清掉了）：横幅自动收起。没有这一条它会一直
  // 挂着，直到用户手动关 —— 而它描述的事实那时早已不成立。
  useEffect(() => {
    if (survivor && !entries.some((e) => isSameProcess(e, survivor.pid, survivor.startUnix))) {
      setSurvivor(null);
    }
  }, [entries, survivor]);

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

  // 弹窗（确认 / 批量）全部关闭后，焦点还给打开它的按钮
  const anyModalOpen = confirm !== null || batchConfirm !== null;
  useEffect(() => {
    if (!anyModalOpen && modalTrigger.current) {
      modalTrigger.current.focus();
      modalTrigger.current = null;
    }
  }, [anyModalOpen]);

  const rememberTrigger = useCallback(() => {
    modalTrigger.current =
      document.activeElement instanceof HTMLElement ? document.activeElement : null;
  }, []);

  const suspectCount = entries.filter((e) => e.is_zombie_suspect).length;
  const sweepables = sweepableEntries(entries);
  const whitelistCount = entries.filter((e) => e.is_whitelisted).length;
  const totalPortCount = entries.reduce((sum, e) => sum + e.ports.length, 0);

  const filtered = entries.filter((e) => {
    if (filter === "suspect" && !e.is_zombie_suspect) return false;
    if (filter === "whitelist" && !e.is_whitelisted) return false;
    if (search) {
      const q = search.toLowerCase();
      // UI 全程以 ":5173" 展示端口 —— 用户原样复制粘贴也要能搜到（评审发现）
      const portQ = q.startsWith(":") ? q.slice(1) : q;
      return (
        e.command.toLowerCase().includes(q) ||
        e.full_command.toLowerCase().includes(q) ||
        e.app_label.toLowerCase().includes(q) ||
        e.launcher_label.toLowerCase().includes(q) ||
        (portQ !== "" && e.ports.some((p) => String(p).includes(portQ))) ||
        String(e.pid).includes(q)
      );
    }
    return true;
  });

  const suspects = filtered.filter((e) => e.is_zombie_suspect);
  const healthy = filtered.filter((e) => !e.is_zombie_suspect);

  // useCallback 化(连同下方 handleToggleWhitelist / handleOpen / toggleExpand):
  // Row 经 memo 缓存,只有当传入的回调引用稳定时 memo 才命中,否则每轮轮询新建的
  // 回调会让所有行重渲染,memo 形同虚设。依赖均为稳定项(setter / 已 useCallback)。
  const askKill = useCallback(
    (e: ProcessEntry, force: boolean) => {
      rememberTrigger();
      setConfirm({
        pid: e.pid,
        // 完整命令行（含脚本路径/参数）—— 毁灭性确认应让用户看清杀的到底是什么，
        // lsof 短名 "node" 不足以辨认（评审发现）；退回短名仅当完整命令为空
        command: e.full_command || e.command,
        ports: e.ports,
        app_label: e.app_label,
        force,
        startUnix: e.start_unix,
      });
    },
    [rememberTrigger],
  );

  // 统一「动作成功 → 清除残留失败横幅；失败 → 设置本次文案」语义。集中一处，
  // 避免每个动作处理器各自手抄 setActionError(null) 而新增的漏写（评审发现：原先 4 处）。
  // toErrorMsg 接受翻译器注入并在渲染时调用 —— 切换语言后横幅自动重译。
  const runAction = useCallback(
    async (work: () => Promise<void>, toErrorMsg: (err: unknown, tr: Translator) => string) => {
      try {
        await work();
        setActionError(null);
      } catch (err) {
        setActionError({ render: (tr) => toErrorMsg(err, tr) });
      }
    },
    [],
  );

  /** 目标是否真的消失了。轮询到 deadline 为止 —— 单次 250ms 后的快照会把
   *  「正在优雅退出」误报成「杀不掉」。身份用 (pid, start_unix) **成对**比较
   *  （只比 PID 的话，PID 被回收后新起的另一个进程会被当成「没杀掉」），且必须
   *  走 `isSameProcess` 的容差比较 —— start_unix 逐轮抖动，`===` 会随机误判。
   *
   *  口径说明：这里的「消失」= **不再出现在扫描结果里**。扫描只列监听端口的进程
   *  与 dev-like 的无端口孤儿，所以一个释放了端口却仍活着的进程也会算「消失」。
   *  对本产品这是可接受的口径（用户要的是端口回来），但别把它当成 `kill(pid,0)`。 */
  const confirmGone = useCallback(
    async (pid: number, startUnix: number): Promise<boolean> => {
      const deadline = Date.now() + CONFIRM_KILL_DEADLINE_MS;
      for (;;) {
        const rows = await freshScan();
        // 扫描失败（返回 null）时不下结论：这一轮没有证据，交给常规轮询
        if (rows === null) return true;
        if (!rows.some((r) => isSameProcess(r, pid, startUnix))) return true;
        if (Date.now() >= deadline) return false;
        await sleep(CONFIRM_KILL_POLL_MS);
      }
    },
    [freshScan],
  );

  /** 批量版的存活确认：整批共用一个 deadline，轮询到全部消失或到期为止。
   *  与 `confirmGone` 同一套语义（同样的容差比较、同样的 2.5s 上限、扫描失败时
   *  不下结论），只是一次判一批 —— 逐个串行调 confirmGone 会把上限乘以目标数。 */
  const confirmGoneAll = useCallback(
    async (targets: ProcessEntry[]): Promise<ProcessEntry[]> => {
      const deadline = Date.now() + CONFIRM_KILL_DEADLINE_MS;
      for (;;) {
        const rows = await freshScan();
        // 扫描失败：这一轮没有证据，不把它当成「都还活着」
        if (rows === null) return [];
        const alive = targets.filter((s) =>
          rows.some((r) => isSameProcess(r, s.pid, s.start_unix)),
        );
        if (alive.length === 0 || Date.now() >= deadline) return alive;
        await sleep(CONFIRM_KILL_POLL_MS);
      }
    },
    [freshScan],
  );

  const doKill = async () => {
    if (!confirm) return;
    const { pid, force, startUnix, app_label, command, ports } = confirm;
    setKillingPids((cur) => new Set(cur).add(pid));
    setConfirm(null);
    setSurvivor(null);
    try {
      await runAction(
        async () => {
          try {
            await invokeAction("kill_process", { pid, force, startUnix });
          } catch (err) {
            // 扫描到点击之间有 0–2s 窗口：dev server 自己退了、清扫刚跑完、
            // 双击了终止键 —— 后端如实返回 process_gone，而用户明明达成了目的。
            // 把它渲染成红色「终止失败」直接损伤「这个工具的判断可以信」的基调。
            if (killErrorCode(err) !== "process_gone") throw err;
          }
          await sleep(250);
          if (startUnix != null && !(await confirmGone(pid, startUnix))) {
            setSurvivor({ pid, label: app_label, startUnix, command, ports });
          }
        },
        // 原值直传（不套 String）：kill 错误是结构化的 {code, message?}，
        // 先转字符串会把它压成 "[object Object]"，分派立刻退化成透传。
        (err, tr) => tr("error.killFailed", { err: localizeKillError(err, tr) }),
      );
    } finally {
      // 只摘掉自己那一个：并发的 kill 各自收尾，互不影响对方的禁用态。
      setKillingPids((cur) => {
        if (!cur.has(pid)) return cur;
        const next = new Set(cur);
        next.delete(pid);
        return next;
      });
    }
  };

  const handleToggleWhitelist = useCallback(
    async (e: ProcessEntry) => {
      // 引擎随每行产出的键，前端不再重推（见 model.ts ProcessEntry.whitelist_key）
      const key = e.whitelist_key;
      await runAction(
        async () => {
          if (e.is_whitelisted) {
            await invokeAction("remove_whitelist", { key });
            // v0.4.0 旧键也清掉，否则升级用户的裸键仍命中、星标取消不掉（评审发现）
            const legacy = legacyWhitelistKey(e);
            if (legacy !== key) await invokeAction("remove_whitelist", { key: legacy });
          } else {
            await invokeAction("add_whitelist", { key });
          }
          // freshScan 而非 refresh：2s 轮询大概率正有一次扫描在飞行中，它读到的是
          // 白名单落盘**之前**的数据；refresh 会复用该 Promise，星标/嫌疑态/清扫
          // 计数要到下一轮才更新（评审发现）。kill 路径同理，早已用 freshScan。
          await freshScan();
        },
        (err, tr) => tr("error.whitelistFailed", { err: localizeActionError(String(err), tr) }),
      );
    },
    [runAction, freshScan],
  );

  const handleOpen = useCallback(
    async (port: number) => {
      await runAction(
        async () => {
          await openUrl(`http://localhost:${port}`);
        },
        (err, tr) => tr("error.openBrowser", { err: String(err) }),
      );
    },
    [runAction],
  );

  const askKillAllSuspects = () => {
    if (sweepables.length === 0) return;
    rememberTrigger();
    setBatchConfirm(sweepables);
  };

  const doBatchKill = async () => {
    // targets 是弹窗打开那一刻的清扫快照 —— 与上面 suspects（当前过滤视图的
    // 渲染子集）语义不同，命名刻意区分（评审发现：曾同名遮蔽）。
    const targets = batchConfirm;
    if (!targets || targets.length === 0) return;
    setBatchConfirm(null);
    setSweeping(true);
    // Windows 无 SIGTERM：单一 TerminateProcess 语义（force）
    const force = os === "windows";
    try {
      await runAction(
        async () => {
          const failures: BatchFailure[] = [];
          for (const s of targets) {
            try {
              await invokeAction("kill_process", {
                pid: s.pid,
                force,
                startUnix: s.start_unix,
              });
            } catch (err) {
              // 目标在清扫开始前已自行退出：那正是清扫想要的结果，不进失败列表
              if (killErrorCode(err) === "process_gone") continue;
              failures.push({ pid: s.pid, label: s.app_label, raw: err });
            }
          }
          await sleep(250);
          // 与单杀**同一条口径、同一个上限**：信号送到了 ≠ 进程死了，而优雅退出要
          // 0.3–2s。此前这里只 sleep(700) 后扫一次 —— 单次快照会把一整批正在收尾的
          // 目标全部读成「没杀掉」，而清扫一次能打十几个，误报会同时命中多行
          // （评审发现：这正是我自己写进 CLAUDE.md 的那条不变量，批量路径却违反了它）。
          const survivors = await confirmGoneAll(targets);
          // 强杀出口需要身份令牌，故提示只能挂在**带 start_unix**的那一行上。
          // 取 survivors[0] 会在它恰好没有令牌时把整条提示连同其余幸存者一起丢掉
          // —— 结果正是本轮要消灭的形态：清扫完还剩几行，用户零反馈（评审发现）。
          const anchor = survivors.find((s) => s.start_unix != null);
          if (anchor?.start_unix != null) {
            setSurvivor({
              pid: anchor.pid,
              label:
                survivors.length === 1
                  ? anchor.app_label
                  : `${anchor.app_label} (+${survivors.length - 1})`,
              startUnix: anchor.start_unix,
              command: anchor.full_command || anchor.command,
              ports: anchor.ports,
            });
          }
          // 部分失败：抛出结构化失败列表，由 toErrorMsg 在渲染时组装并本地化
          if (failures.length > 0) throw failures;
        },
        (msg, tr) => {
          if (Array.isArray(msg)) {
            const fails = msg as BatchFailure[];
            return (
              tr("error.batchFailed", {
                failed: fails.length,
                total: targets.length,
              }) +
              // 分隔符语言无关（评审发现：全角「；」会出现在英文界面）
              fails
                .map((f) => `PID ${f.pid} ${f.label} (${localizeKillError(f.raw, tr)})`)
                .join("; ")
            );
          }
          return String(msg);
        },
      );
    } finally {
      setSweeping(false);
    }
  };

  const toggleExpand = useCallback(
    (pid: number) => setExpandedPid((cur) => (cur === pid ? null : pid)),
    [],
  );

  // useMemo:回调已稳定,shared 只在 os/lang/killingPids/sweeping 变化时新建 ——
  // Row 的 memo 按它的对象身份比较(见 ProcessRow rowPropsEqual),2s 轮询不再
  // 无谓重渲染全表。这里的依赖数组是 shared 唯一的手工维护点。
  // killingPids 是不可变 Set:每次增删都换新身份,memo 才会跟着失效。
  const shared = useMemo(
    () => ({
      os,
      lang,
      killingPids,
      sweeping,
      onAskKill: askKill,
      onToggleWhitelist: handleToggleWhitelist,
      onOpenPort: handleOpen,
      onToggleExpand: toggleExpand,
    }),
    [os, lang, killingPids, sweeping, askKill, handleToggleWhitelist, handleOpen, toggleExpand],
  );

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <svg
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
            aria-label={t("search.placeholder")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            className="search"
          />
        </div>

        <div className="filter-tabs">
          <button
            className={filter === "all" ? "active" : ""}
            aria-pressed={filter === "all"}
            onClick={() => setFilter("all")}
          >
            {t("filter.all")}
            <span className="tab-count">{entries.length}</span>
          </button>
          <button
            className={`${filter === "suspect" ? "active" : ""} ${suspectCount > 0 ? "has-suspects" : ""}`}
            aria-pressed={filter === "suspect"}
            onClick={() => setFilter("suspect")}
          >
            {t("filter.suspect")}
            <span className="tab-count">{suspectCount}</span>
          </button>
          <button
            className={filter === "whitelist" ? "active" : ""}
            aria-pressed={filter === "whitelist"}
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
              // 单杀飞行中禁止发起清扫（与「清扫中禁用行内终止」互为镜像）：
              // 清扫快照仍含正被杀的 PID，二次 kill 只会制造多余的失败横幅（评审发现）
              disabled={sweeping || killingPids.size > 0}
              onClick={askKillAllSuspects}
              title={t("sweep.title")}
            >
              {sweeping ? t("sweep.sweeping") : `${t("sweep.button")} (${sweepables.length})`}
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

      {/* alert 与 button 语义不能同处一元素（评审发现）：外层 live region 负责
          读屏播报，内层保留可点击关闭。区域常驻挂载 —— 内容注入既有 alert 才可靠触发播报 */}
      <div className="error-region" role="alert">
        {error && (
          <div
            className="error"
            role="button"
            tabIndex={0}
            onClick={dismissError}
            onKeyDown={(ev) => {
              if (ev.key === "Enter" || ev.key === " ") {
                ev.preventDefault();
                dismissError();
              }
            }}
          >
            {error} {t("error.clickToClose")}
          </div>
        )}
      </div>

      {/* 「信号送到了，但进程没死」——独立于错误横幅的一路提示：中性色 + 一个
          可操作的出口。role="status" 而非 alert：这不是错误，只是结果与预期不符。
          区域常驻挂载、内容后注入 —— 与上面的 error-region 同一条教训：live region
          与内容同时出现时读屏不会播报。 */}
      <div className="notice-region" role="status">
        {survivor && (
          <div className="notice">
            <span>{t("kill.survivor", { pid: survivor.pid, label: survivor.label })}</span>
            {os === "macos" && (
              <button
                className="btn-ghost"
                onClick={() => {
                  rememberTrigger();
                  // 直接用横幅自带的信息重开确认框，不去 entries 里捞 ——
                  // 捞不到时按钮会静默失效，而那正是最需要它工作的时刻
                  setConfirm({
                    pid: survivor.pid,
                    command: survivor.command,
                    ports: survivor.ports,
                    app_label: survivor.label,
                    force: true,
                    startUnix: survivor.startUnix,
                  });
                  setSurvivor(null);
                }}
              >
                {t("kill.survivor.force")}
              </button>
            )}
            <button className="btn-ghost" onClick={() => setSurvivor(null)}>
              {t("kill.survivor.dismiss")}
            </button>
          </div>
        )}
      </div>

      <main className="list">
        {/* 空态分四种，此前全部塌缩成一句「没有发现任何监听端口」：
            ① 还没扫完 —— 每次启动的第一帧都会经过这里，宣布「没有端口」是假话；
            ② 首轮扫描失败 —— entries 仍是 []，同一句话会被自信地说出来，
               而此时真正需要的是一个重试入口（界面上此前根本没有手动刷新）；
            ③ 收藏页为空 —— 那是「你还没收藏过」，不是「没有匹配项」，
               也是解释 ★ 语义的最佳时机；④ 才是真的一台干净机器。 */}
        {!hasScanned ? (
          <div className="empty">{t("empty.scanning")}</div>
        ) : entries.length === 0 && !lastScanOk ? (
          <div className="empty">
            {t("empty.scanFailed")}
            <button className="btn-ghost empty-action" onClick={() => void freshScan()}>
              {t("empty.retry")}
            </button>
          </div>
        ) : filter === "whitelist" && whitelistCount === 0 ? (
          <div className="empty">{t("empty.noStarred")}</div>
        ) : entries.length === 0 ? (
          <div className="empty">{t("empty.none")}</div>
        ) : filtered.length === 0 && !(filter === "suspect" && !search) ? (
          // 「可疑」标签页下无搜索词且零嫌疑 ⇒ 不是「没有匹配项」而是「一切正常」，
          // 落入下方分支由 allclear 呈现（评审发现）
          <div className="empty">{t("empty.noMatch")}</div>
        ) : (
          <>
            {filter !== "whitelist" && suspects.length > 0 && (
              <Section
                title={t("section.suspects")}
                sub={t("section.suspects.sub")}
                danger
                entries={suspects}
                expandedPid={expandedPid}
                shared={shared}
              />
            )}

            {/* 搜索中不下全局结论：suspects 来自过滤后的子集，机器上可能正有
                嫌疑进程只是没匹配搜索词 —— 那时说「一切正常」是误导（评审发现） */}
            {filter !== "whitelist" && !search && suspects.length === 0 && (
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
              <Section
                title={t("section.healthy")}
                entries={healthy}
                expandedPid={expandedPid}
                shared={shared}
              />
            )}

            {filter === "whitelist" && (
              <Section
                title={t("section.starred")}
                entries={filtered}
                expandedPid={expandedPid}
                shared={shared}
              />
            )}
          </>
        )}
      </main>

      <footer className="footer">
        {t("footer.status", { procs: entries.length, ports: totalPortCount })}
        {/* 版本号在此前的 UI 里完全不可达：应用是 Accessory（无 Dock 图标），
            那份带「关于」的应用菜单根本不显示 —— 用户报 issue 时说不清自己在用
            哪一版。刻意单独一个 span，不拼进 i18n 的值里。
            取自 vite define（bump-version.mjs 已同步 package.json，不会漂移）。 */}
        <span className="footer-version" title="Portreaper">
          v{__APP_VERSION__}
        </span>
      </footer>

      {batchConfirm && (
        <BatchConfirmModal
          targets={batchConfirm}
          os={os}
          onCancel={() => setBatchConfirm(null)}
          onConfirm={doBatchKill}
        />
      )}

      {confirm && (
        <KillConfirmModal
          confirm={confirm}
          os={os}
          onCancel={() => setConfirm(null)}
          onConfirm={doKill}
        />
      )}
    </div>
  );
}

export default App;
