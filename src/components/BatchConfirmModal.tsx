import { useI18n } from "../i18n";
import { formatPorts, type Os, type ProcessEntry } from "../model";
import { ConfirmModal } from "./ConfirmModal";

/** 批量清扫确认弹窗（纯展示，从 App.tsx 抽出）：信号说明 + 前 8 个目标预览 +
 *  清扫范围注记（仅 Confirmed + Likely —— CLAUDE.md 不变量，文案与行内层级名严格一致）。 */
export function BatchConfirmModal(props: {
  targets: ProcessEntry[];
  os: Os | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  const { targets, os } = props;
  return (
    <ConfirmModal
      titleId="batch-modal-title"
      title={t("batch.title", { n: targets.length })}
      cancelLabel={t("batch.cancel")}
      confirmLabel={t("batch.confirm")}
      onCancel={props.onCancel}
      onConfirm={props.onConfirm}
    >
      <div className="modal-row">
        <span className="modal-label">{t("batch.signal")}</span>
        <span className="mono">
          {os === "windows" ? t("batch.signal.windows") : t("batch.signal.macos")}
        </span>
      </div>
      <div className="modal-row modal-row-top">
        <span className="modal-label">{t("batch.procs")}</span>
        <div className="batch-list">
          {targets.slice(0, 8).map((s) => (
            <div key={s.pid} className="batch-item mono">
              <span className="batch-pid">PID {s.pid}</span>
              <span className="batch-label">{s.app_label}</span>
              <span className="batch-ports muted">{formatPorts(s.ports)}</span>
            </div>
          ))}
          {targets.length > 8 && (
            <div className="muted batch-more">{t("batch.more", { n: targets.length - 8 })}</div>
          )}
        </div>
      </div>
      <div className="modal-row">
        <span className="muted">{t("batch.scope.note")}</span>
      </div>
    </ConfirmModal>
  );
}
