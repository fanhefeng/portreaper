import { useI18n } from "../i18n";
import type { Translator } from "../model";
import { localizeActionError } from "../model";
import type { UpdaterState } from "../updater";
import { trapTab } from "./ConfirmModal";

/** 下载进度的人话：有 Content-Length 给百分比，没有就给已下载的 MB 数。 */
export function downloadProgressText(
  downloaded: number,
  total: number | null,
  t: Translator,
): string {
  if (total && total > 0) {
    const pct = Math.min(100, Math.floor((downloaded / total) * 100));
    return t("update.downloading", { pct });
  }
  return t("update.downloadingNoTotal", { mb: (downloaded / (1024 * 1024)).toFixed(1) });
}

/**
 * 更新弹窗：available（确认安装）→ downloading/installing（进度，不可关闭）
 * → installed（重启确认）/ installFailed（可重试）。
 *
 * 刻意不复用 ConfirmModal 骨架：那是毁灭性确认（红色主按钮 + 取消/确认双按钮）
 * 的形状，更新不是毁灭性操作，按钮组也随阶段变化 —— 只共享 trapTab 与 CSS 类。
 * 下载/安装期间没有任何关闭出口（backdrop 点击 / Esc 都不关）：取消不了正在
 * 落盘的安装，一个「装到一半还能点走」的弹窗只会撒谎。
 */
export function UpdateModal(props: {
  state: UpdaterState;
  onClose: () => void;
  onInstall: () => void;
  onRestart: () => void;
  onOpenReleasePage: (version: string) => void;
}) {
  const { t } = useI18n();
  const { state } = props;
  if (
    state.phase !== "available" &&
    state.phase !== "downloading" &&
    state.phase !== "installing" &&
    state.phase !== "installed" &&
    state.phase !== "installFailed"
  ) {
    return null;
  }
  const closable = state.phase !== "downloading" && state.phase !== "installing";
  const title =
    state.phase === "available"
      ? t("update.title")
      : state.phase === "installed"
        ? t("update.installedTitle")
        : state.phase === "installFailed"
          ? t("update.failedTitle")
          : t("update.installingTitle");

  return (
    <div className="modal-backdrop" onClick={closable ? props.onClose : undefined}>
      <div
        className="modal update-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="update-modal-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={trapTab}
      >
        <div className="modal-title" id="update-modal-title">
          {title}
        </div>
        <div className="modal-body">
          <div className="update-versions">
            {t("update.versionLine", {
              current: state.info.current_version,
              next: state.info.version,
            })}
          </div>

          {state.phase === "available" && state.info.notes && (
            <div className="update-notes">{state.info.notes}</div>
          )}

          {(state.phase === "downloading" || state.phase === "installing") && (
            <div className="update-progress">
              <div
                className="update-progress-track"
                role="progressbar"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={
                  state.phase === "downloading" && state.total
                    ? Math.min(100, Math.floor((state.downloaded / state.total) * 100))
                    : undefined
                }
              >
                <div
                  className={`update-progress-fill${
                    state.phase === "installing" || (state.phase === "downloading" && !state.total)
                      ? " indeterminate"
                      : ""
                  }`}
                  style={
                    state.phase === "downloading" && state.total
                      ? {
                          width: `${Math.min(100, (state.downloaded / state.total) * 100)}%`,
                        }
                      : undefined
                  }
                />
              </div>
              <div className="update-progress-text" role="status">
                {state.phase === "installing"
                  ? t("update.installing")
                  : downloadProgressText(state.downloaded, state.total, t)}
              </div>
            </div>
          )}

          {state.phase === "installed" && <div>{t("update.installedBody")}</div>}

          {state.phase === "installFailed" && (
            <div className="update-error">
              {t("update.failedBody", { err: localizeActionError(state.message, t) })}
            </div>
          )}
        </div>
        <div className="modal-actions">
          {state.phase === "available" && (
            <>
              <button
                className="btn-ghost update-release-link"
                onClick={() => props.onOpenReleasePage(state.info.version)}
              >
                {t("update.releasePage")}
              </button>
              <button className="btn-ghost" autoFocus onClick={props.onClose}>
                {t("update.later")}
              </button>
              <button className="btn-accent-solid" onClick={props.onInstall}>
                {t("update.install")}
              </button>
            </>
          )}
          {state.phase === "installed" && (
            <>
              <button className="btn-ghost" onClick={props.onClose}>
                {t("update.later")}
              </button>
              <button className="btn-accent-solid" autoFocus onClick={props.onRestart}>
                {t("update.restart")}
              </button>
            </>
          )}
          {state.phase === "installFailed" && (
            <>
              <button className="btn-ghost" autoFocus onClick={props.onClose}>
                {t("update.close")}
              </button>
              <button className="btn-accent-solid" onClick={props.onInstall}>
                {t("update.retry")}
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
