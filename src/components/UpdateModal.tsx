import { useEffect, useRef } from "react";
import { useI18n } from "../i18n";
import type { Translator } from "../model";
import { localizeActionError } from "../model";
import type { UpdaterState } from "../updater";
import { trapTab } from "./ConfirmModal";

/**
 * 下载百分比（0–100 的整数）；服务器没给 Content-Length 时为 null（= 不确定态）。
 *
 * 三个消费点共用：进度文案、进度条宽度、`aria-valuenow`。此前三处各算一遍，
 * 且宽度那份**没取整** —— 同一时刻文案说 99%、进度条画 99.6%、读屏播报走第三份
 * 计算（评审发现）。null 同时也是「画不确定态」的判据，省掉一份平行条件。
 */
export function downloadPercent(downloaded: number, total: number | null): number | null {
  if (!total || total <= 0) return null;
  // 双侧夹紧：函数名承诺的是百分比，两个消费点也都要求 0–100（CSS 宽度、
  // aria-valuenow 的 min/max）。上界防「服务器给的 Content-Length 偏小」，
  // 下界纯属让函数对齐它的契约 —— 累计字节数不该为负，但边界由这里保证，
  // 而不是靠调用方相信上游。
  return Math.min(100, Math.max(0, Math.floor((downloaded / total) * 100)));
}

/** 下载进度的人话：有 Content-Length 给百分比，没有就给已下载的 MB 数。 */
export function downloadProgressText(
  downloaded: number,
  total: number | null,
  t: Translator,
): string {
  const pct = downloadPercent(downloaded, total);
  if (pct !== null) return t("update.downloading", { pct });
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
  // 只算一次，下面三处（宽度 / aria-valuenow / 文案）共用；null = 不确定态
  const pct = state.phase === "downloading" ? downloadPercent(state.downloaded, state.total) : null;
  // 下载/安装阶段整个弹窗一个按钮都没有（modal-actions 只在另外三个阶段渲染）。
  // 那时焦点会掉回 <body>，keydown 再也传不到弹窗上 —— trapTab 收不到事件，Tab
  // 一路走进背后的列表，可以在 aria-modal 弹窗后面按到「终止」（评审发现）。
  // 把焦点收到容器上，圈定才重新生效。
  const noActions = !closable;
  const bodyRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (noActions) bodyRef.current?.focus();
  }, [noActions]);
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
        ref={bodyRef}
        // 让容器可编程聚焦（不进 Tab 序列）：无按钮阶段焦点收到这里，
        // onKeyDown 才收得到 Tab，trapTab 才拦得住（见上方 noActions 注释）
        tabIndex={-1}
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
                aria-valuenow={pct ?? undefined}
              >
                <div
                  className={`update-progress-fill${pct === null ? " indeterminate" : ""}`}
                  style={pct === null ? undefined : { width: `${pct}%` }}
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
