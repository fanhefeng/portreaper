import type { KeyboardEvent as ReactKeyboardEvent, ReactNode } from "react";

/** 毁灭性确认弹窗的共用骨架：backdrop 点击关闭、stopPropagation、dialog aria
 *  语义、Tab 焦点圈定、autoFocus 落在取消键（Enter 永远不应直接触发终止）。
 *  单杀与批量两处共用 —— 这些 a11y/安全不变量集中一处维护，
 *  不再随两份手抄脚手架漂移（评审发现）。 */
export function ConfirmModal(props: {
  titleId: string;
  title: string;
  cancelLabel: string;
  confirmLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
  children: ReactNode;
}) {
  return (
    <div className="modal-backdrop" onClick={props.onCancel}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={props.titleId}
        onClick={(e) => e.stopPropagation()}
        onKeyDown={trapTab}
      >
        <div className="modal-title" id={props.titleId}>
          {props.title}
        </div>
        <div className="modal-body">{props.children}</div>
        <div className="modal-actions">
          <button className="btn-ghost" autoFocus onClick={props.onCancel}>
            {props.cancelLabel}
          </button>
          <button className="btn-danger-solid" onClick={props.onConfirm}>
            {props.confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 极简焦点圈定：Tab 在弹窗内的按钮间循环（毁灭性确认弹窗的键盘安全网）。
 *  UpdateModal 也复用它 —— 弹窗骨架不同（非毁灭性、无固定双按钮），但
 *  焦点不该逃出弹窗这条 a11y 不变量是同一条。 */
export function trapTab(ev: ReactKeyboardEvent<HTMLDivElement>) {
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
