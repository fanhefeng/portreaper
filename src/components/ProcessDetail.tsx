import { categoryKey, reasonKey, reasonTipKey, useI18n } from "../i18n";
import {
  exemptReasons,
  formatDuration,
  formatPorts,
  subtreeCpuExceedsSelf,
  type Os,
  type ProcessEntry,
} from "../model";
import { splitLabel } from "../describe";

/** 展开行的详情面板：命令 / 路径 / 资源 / 启动链 + 判定证据与豁免理由。 */
export function ProcessDetail({ e, os, id }: { e: ProcessEntry; os: Os | null; id: string }) {
  const { t } = useI18n();

  // 链末节点用主名（app_label 可能带 " · node" 次级说明，链里不需要）
  const selfName = splitLabel(e.app_label).name;

  // 合法类别清单由 i18n 字典本身承担（cat.* 键族），未知类别落 cat.unknown
  const catKey = categoryKey(e.app_category);

  const exempt = exemptReasons(e);

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
          {e.ports.length > 0 ? formatPorts(e.ports, "  ") : "—"}
        </span>

        <span className="detail-label">{t("detail.pid")}</span>
        <span className="detail-value mono">
          {e.pid}
          <span className="detail-sep">·</span>
          <span className="detail-dim">
            {t("detail.parent")} {e.ppid}
          </span>
          {/* user / tty 是「会话已死」等判定的可读佐证，契约字段不再只收不显（评审发现） */}
          {e.user && (
            <>
              <span className="detail-sep">·</span>
              <span className="detail-dim">{e.user}</span>
            </>
          )}
          {e.tty && e.tty !== "??" && (
            <>
              <span className="detail-sep">·</span>
              <span className="detail-dim">{e.tty}</span>
            </>
          )}
          {/* ps state 标志（如 S+ / Z）：defunct 等判定的原始佐证，
              与 user/tty 同理不再只收不显（评审发现） */}
          {e.state && (
            <>
              <span className="detail-sep">·</span>
              <span className="detail-dim" title={t("detail.state")}>
                {e.state}
              </span>
            </>
          )}
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
          {/* 子树合计只在「CPU 烧在子进程里」时追加：无头浏览器把负载全放在
              gpu-process 子进程，主进程行显示 ~0% —— 不露出来用户无从发现
              一棵空转的进程树（KNOWN-GAPS Gap 1/B）。等值时不重复噪音。 */}
          {subtreeCpuExceedsSelf(e) && (
            <span className="detail-dim" title={t("detail.resources.tree.tip")}>
              {" "}
              {t("detail.resources.tree", { cpu: e.cpu_percent_tree.toFixed(1) })}
            </span>
          )}
        </span>

        <span className="detail-label">{t("detail.chain")}</span>
        <span className="detail-value">
          {chainTopDown.length === 0 ? (
            <span className="detail-dim">{t("detail.chain.empty")}</span>
          ) : (
            <span className="chain">
              {chainTopDown.map((p, i) => (
                // title 悬浮出示节点的 PID 与可执行路径 —— 链上父进程的辨认依据，
                // ParentRef 契约字段不再只收不显（评审发现）
                <span
                  className="chain-node"
                  key={`${p.pid}-${i}`}
                  title={p.exe_path ? `PID ${p.pid} · ${p.exe_path}` : `PID ${p.pid}`}
                >
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
          <div className="evidence-title evidence-title-danger">{t("detail.evidence")}</div>
          {e.zombie_reasons.map((r) => (
            <div className="evidence-item" key={r}>
              <span className="evidence-name">
                {t(reasonKey(r))}
                {/* 重复实例的可操作目标（对端 PID）必须在详情里可见（评审发现） */}
                {r === "duplicate_dev_server" && e.duplicate_of != null && (
                  <span className="detail-dim"> · PID {e.duplicate_of}</span>
                )}
              </span>
              <span className="evidence-text">{t(reasonTipKey(r))}</span>
            </div>
          ))}
        </div>
      )}

      {exempt.length > 0 && (
        <div className="evidence">
          <div className="evidence-title">{t("detail.whyNot")}</div>
          {exempt.map((r) => (
            <div className="evidence-item" key={r}>
              <span className="evidence-name evidence-name-ok">✓ {t(reasonKey(r))}</span>
              <span className="evidence-text">{t(reasonTipKey(r))}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
