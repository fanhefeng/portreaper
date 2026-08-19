import { useI18n } from "../i18n";
import { formatPorts, type Os } from "../model";
import { ConfirmModal } from "./ConfirmModal";

/** 单杀确认的载荷：App 在用户点击行内终止时从 ProcessEntry 摘取的快照
 *（弹窗期间行可能随轮询消失，故不持有 entry 引用而是拷贝所需字段）。 */
export type KillConfirm = {
  pid: number;
  command: string;
  ports: number[];
  app_label: string;
  force: boolean;
  startUnix: number | null;
};

/** 单杀确认弹窗（纯展示，从 App.tsx 抽出）：应用 / 完整命令 / PID / 端口 / 信号。
 *  信号文案是平台分叉的 —— Windows 恒为 TerminateProcess（force 参数被后端忽略）。 */
export function KillConfirmModal(props: {
  confirm: KillConfirm;
  os: Os | null;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  const { confirm, os } = props;
  const forceOnMac = confirm.force && os !== "windows";
  return (
    <ConfirmModal
      titleId="confirm-modal-title"
      title={forceOnMac ? t("confirm.title.force") : t("confirm.title.kill")}
      cancelLabel={t("confirm.cancel")}
      confirmLabel={forceOnMac ? t("confirm.force") : t("confirm.kill")}
      onCancel={props.onCancel}
      onConfirm={props.onConfirm}
    >
      <div className="modal-row">
        <span className="modal-label">{t("confirm.app")}</span>
        <span>{confirm.app_label}</span>
      </div>
      <div className="modal-row">
        <span className="modal-label">{t("confirm.cmd")}</span>
        <span className="mono modal-cmd">{confirm.command}</span>
      </div>
      <div className="modal-row">
        <span className="modal-label">{t("confirm.pid")}</span>
        <span className="mono">{confirm.pid}</span>
      </div>
      <div className="modal-row">
        <span className="modal-label">{t("confirm.ports")}</span>
        <span className="mono">
          {formatPorts(confirm.ports, "  ")}
          {confirm.ports.length > 1 && (
            <span className="muted"> {t("confirm.portsRelease", { n: confirm.ports.length })}</span>
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
    </ConfirmModal>
  );
}
