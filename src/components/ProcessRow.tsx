import { memo, useMemo } from "react";
import { reasonKey, storyKey, useI18n, verdictKey, type Lang } from "../i18n";
import {
  exemptReasons,
  formatDuration,
  formatPorts,
  formatUptime,
  hasBusySubtree,
  primaryReason,
  type Os,
  type ProcessEntry,
} from "../model";
import { DESC_KEYS, describeEntry, splitLabel } from "../describe";
import { ProcessDetail } from "./ProcessDetail";

/** 整表共享的行上下文：App 用 useMemo 保持引用稳定（回调全部 useCallback），
 *  只在 os/lang/killingPid/sweeping 变化时新建 —— memo 比较靠它的**对象身份**。 */
export type RowShared = {
  os: Os | null;
  lang: Lang;
  killingPid: number | null;
  sweeping: boolean;
  onAskKill: (e: ProcessEntry, force: boolean) => void;
  onToggleWhitelist: (e: ProcessEntry) => void;
  onOpenPort: (port: number) => void;
  onToggleExpand: (pid: number) => void;
};

export type RowProps = {
  e: ProcessEntry;
  expanded: boolean;
  shared: RowShared;
};

// Row 用 memo + 自定义比较:轮询每 2s 产生全新 entries 数组,e 引用必变,默认
// 浅比较失效;这里对 e 做内容比较(serde 字段序固定 → JSON.stringify 稳定),
// shared 按对象身份比较 —— App 的 useMemo 保证只有 os/lang/killingPid/sweeping
// 真正变化时才换新对象。仅当本行数据或共享上下文变化才重渲染(否则一行 ~50 条
// 正则 + 整棵 JSX 每 2s 白跑)。
//
// shared 收敛成单对象是刻意的(评审发现):此前 8 个散 prop 由比较器逐字段手抄,
// 往 RowProps 加 prop 而忘改比较器时,表现是无告警的静默 stale 渲染 —— 与下面
// JSON.stringify 防的是同一类陷阱,只是坑挪到了比较器自身的字段清单上。收敛后
// 新增回调/标志自动被对象身份覆盖,手工维护点只剩 App 的 useMemo 依赖数组一处。
//
// JSON.stringify 是**刻意保留**的,不要"优化"成手写逐字段深比较(复审结论):
// ProcessEntry 有 ports / zombie_reasons / parent_chain 三层嵌套(末者还是对象
// 数组),手写比较漏掉任意一个字段,表现就是"后端数据变了而这一行不重渲染"——
// 一个没有任何报错、只会让 UI 静默停在陈旧状态的 bug,且新增契约字段时必然复发。
// 换来的收益是亚毫秒级:几十行 × 每 2s 两次 ~1KB 序列化。拿正确性换这个不划算。
export const ProcessRow = memo(RowImpl, rowPropsEqual);

function rowPropsEqual(a: RowProps, b: RowProps): boolean {
  return (
    a.expanded === b.expanded &&
    a.shared === b.shared &&
    JSON.stringify(a.e) === JSON.stringify(b.e)
  );
}

function RowImpl({ e, expanded, shared }: RowProps) {
  const {
    os,
    lang,
    killingPid,
    sweeping,
    onAskKill,
    onToggleWhitelist,
    onOpenPort,
    onToggleExpand,
  } = shared;
  const { t } = useI18n();

  // app_label 形如 "dev-server.js · node" —— 主名 + 次级说明
  const { name, sub: nameSub } = splitLabel(e.app_label);

  // describeEntry 跑 ~50 条正则；Row 每 2s 随轮询重渲染。依赖实际输入字段（而非
  // e 引用 —— 每次 poll 都是新对象），值不变即命中缓存（评审 E5）。
  const known = useMemo(
    () => describeEntry(e, lang),
    // eslint 缺席下的手工依赖收窄（评审知情）：describeEntry 只读这五个字段
    [e.app_label, e.command, e.full_command, e.exe_path, e.app_category, lang],
  );
  const desc = known ?? t(DESC_KEYS[e.app_category] ?? "desc.unknown");

  // 来源：谁启动的 / 谁在托管
  const exempt = exemptReasons(e);
  let provenance: string | null = null;
  if (exempt.includes("launchd_managed")) {
    provenance = t("story.managedBySystem");
  } else if (exempt.length > 0 && exempt[0] !== "installed_app") {
    provenance = t(reasonKey(exempt[0]));
  } else if (e.launcher_label && e.launcher_label !== "?") {
    provenance =
      e.launcher_label === "launchd"
        ? t("story.launchedBySystem")
        : t("story.launchedBy", { app: e.launcher_label });
  }

  const primary = e.is_zombie_suspect ? primaryReason(e.zombie_reasons) : null;
  const shownPorts = e.ports.slice(0, 3);
  const morePorts = e.ports.length - shownPorts.length;
  // 清扫进行中禁用全部行内终止按钮（评审发现）：批量循环正逐个 kill，
  // 此时对同一进程发起第二次 kill 只会制造一条多余的失败横幅
  const killing = killingPid === e.pid || sweeping;
  // ★/☆ 字形不构成可用的读屏按钮名（会读作 "black star"）——
  // aria-label 与 title 同源，收藏/取消收藏的动作语义跟随语言（评审发现）
  const starTip = e.is_whitelisted ? t("star.remove.tip") : t("star.add.tip");

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
              {e.ports.length === 0 ? (
                // 孤儿进程不监听端口：用徽标占位，避免端口列空白让人误以为数据缺失
                <span className="port-none" title={t("row.noPort.tip")}>
                  {t("row.noPort")}
                </span>
              ) : (
                <>
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
                    <span className="port-more" title={formatPorts(e.ports)}>
                      +{morePorts}
                    </span>
                  )}
                </>
              )}
            </span>
            {/* 「负载烧在子进程里」徽标：本行自身看着是闲的，子树却在满核 ——
                无头浏览器的 gpu-process 是典型（KNOWN-GAPS Gap 1）。自身就在
                满核的健康构建不满足条件，不会挂徽标，故不制造日常噪音。 */}
            {hasBusySubtree(e) && (
              <span className="cpu-hot mono" title={t("row.busySubtree.tip")}>
                {t("row.busySubtree", { cpu: Math.round(e.cpu_percent_tree) })}
              </span>
            )}
          </div>
          <div className="row-desc">
            {e.is_zombie_suspect ? (
              <>
                <span className={`verdict verdict-${e.confidence}`}>
                  {t(verdictKey(e.confidence))}
                </span>
                {primary && (
                  <span className="desc-text">
                    {" · "}
                    {t(
                      storyKey(primary),
                      // duplicate 故事需要对端 PID 插值；其余 key 无占位符，参数无害
                      { pid: e.duplicate_of ?? "?" },
                    )}
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
                {provenance && <span className="desc-text desc-dim"> · {provenance}</span>}
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
          {/* 终止按钮的布局是平台分叉的 —— get_platform 落定（os 非 null）后才渲染 */}
          {os === "macos" && (
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
          )}
          {os === "windows" && (
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
            title={starTip}
            aria-label={starTip}
          >
            {e.is_whitelisted ? "★" : "☆"}
          </button>
        </div>
      </div>

      {expanded && <ProcessDetail e={e} os={os} id={`proc-detail-${e.pid}`} />}
    </div>
  );
}
