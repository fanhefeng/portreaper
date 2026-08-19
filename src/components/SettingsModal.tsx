import { useI18n, type Lang } from "../i18n";
import { SCAN_INTERVAL_CHOICES, useSettings, type Appearance } from "../settings";
import { trapTab } from "./ConfirmModal";

/** 语言选项用各自的原生名，刻意不进 i18n 字典：语言名在任何界面语言下都写
 *  自己（「中文」不会变成 "Chinese"）—— 与 header 上 lang-toggle 的字面
 *  "EN"/"中" 同一先例。 */
const LANG_OPTIONS: Array<{ value: Lang; label: string }> = [
  { value: "zh", label: "中文" },
  { value: "en", label: "English" },
];

const APPEARANCE_OPTIONS: Appearance[] = ["system", "dark", "light"];

/**
 * 设置弹窗：语言 / 外观 / 扫描间隔 / 自动检查更新。
 *
 * 所有更改即时生效（macOS 设置惯例，没有「确定/取消」双按钮）——「完成」只是
 * 关闭。与 UpdateModal 同一骨架取舍：非毁灭性，不复用 ConfirmModal（那是红色
 * 主按钮 + 双按钮的形状），只共享 trapTab 与 .modal CSS 类。
 * 控件全部是 <button>（分段选择 + role="switch"），trapTab 的按钮圈定天然覆盖。
 */
export function SettingsModal(props: { onClose: () => void }) {
  const { t, lang, setLang } = useI18n();
  const { scanIntervalSecs, autoCheckUpdates, appearance, update } = useSettings();

  return (
    <div className="modal-backdrop" onClick={props.onClose}>
      <div
        className="modal settings-modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby="settings-modal-title"
        onClick={(e) => e.stopPropagation()}
        onKeyDown={trapTab}
      >
        <div className="modal-title" id="settings-modal-title">
          {t("settings.title")}
        </div>
        <div className="modal-body">
          <div className="settings-row">
            <span className="settings-label" id="settings-lang-label">
              {t("settings.language")}
            </span>
            <div className="segmented" role="group" aria-labelledby="settings-lang-label">
              {LANG_OPTIONS.map((o) => (
                <button
                  key={o.value}
                  className={lang === o.value ? "active" : ""}
                  aria-pressed={lang === o.value}
                  onClick={() => setLang(o.value)}
                >
                  {o.label}
                </button>
              ))}
            </div>
          </div>

          <div className="settings-row">
            <span className="settings-label" id="settings-appearance-label">
              {t("settings.appearance")}
            </span>
            <div className="segmented" role="group" aria-labelledby="settings-appearance-label">
              {APPEARANCE_OPTIONS.map((o) => (
                <button
                  key={o}
                  className={appearance === o ? "active" : ""}
                  aria-pressed={appearance === o}
                  onClick={() => update({ appearance: o })}
                >
                  {t(`settings.appearance.${o}`)}
                </button>
              ))}
            </div>
          </div>

          <div className="settings-row">
            <span className="settings-label" id="settings-interval-label">
              {t("settings.scanInterval")}
            </span>
            <div className="segmented" role="group" aria-labelledby="settings-interval-label">
              {SCAN_INTERVAL_CHOICES.map((n) => (
                <button
                  key={n}
                  className={scanIntervalSecs === n ? "active" : ""}
                  aria-pressed={scanIntervalSecs === n}
                  onClick={() => update({ scanIntervalSecs: n })}
                >
                  {t("settings.scanInterval.option", { n })}
                </button>
              ))}
            </div>
          </div>

          <div className="settings-row">
            <span className="settings-label">
              {t("settings.autoCheck")}
              <span className="settings-note">{t("settings.autoCheck.note")}</span>
            </span>
            <button
              className="switch"
              role="switch"
              aria-checked={autoCheckUpdates}
              aria-label={t("settings.autoCheck")}
              onClick={() => update({ autoCheckUpdates: !autoCheckUpdates })}
            />
          </div>
        </div>
        <div className="modal-actions">
          <button className="btn-ghost" autoFocus onClick={props.onClose}>
            {t("settings.done")}
          </button>
        </div>
      </div>
    </div>
  );
}
