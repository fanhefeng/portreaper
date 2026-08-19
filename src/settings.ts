// 应用设置：localStorage 持久化 + useSyncExternalStore 订阅（模式照抄 i18n.ts）。
// 语言不在这里 —— 它有自己的存储与托盘同步逻辑（i18n.ts），设置面板只是复用其 setLang。
//
// 设置是 GUI 单前端的偏好，刻意**不**进 portreaper-core 的共享持久层：
// whitelist 在 core 是因为三个前端要共享，这里没有那样的消费者。
import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";

export type Appearance = "system" | "dark" | "light";

/** 扫描间隔的可选档位（秒）。给档位而非自由输入：Windows 的 CPU 采样区间就是
 *  两次扫描之间的间隔（commands.rs ScannerState），过短会放大采样噪声，
 *  过长会让「僵尸出现 → 被看见」的延迟失去意义。 */
export const SCAN_INTERVAL_CHOICES = [1, 2, 3, 5, 10] as const;
export type ScanIntervalSecs = (typeof SCAN_INTERVAL_CHOICES)[number];

export type Settings = {
  scanIntervalSecs: ScanIntervalSecs;
  autoCheckUpdates: boolean;
  appearance: Appearance;
};

/** appearance 默认 dark 而非 system：亮色主题是后加的，升级用户在没碰过设置时
 *  看到的必须还是原来的样子 —— 跟随系统会让浅色模式的机器在升级后突然换肤。 */
const DEFAULTS: Settings = {
  scanIntervalSecs: 2,
  autoCheckUpdates: true,
  appearance: "dark",
};

const STORAGE_KEY = "portreaper.settings";

function isScanInterval(v: unknown): v is ScanIntervalSecs {
  return typeof v === "number" && (SCAN_INTERVAL_CHOICES as readonly number[]).includes(v);
}

function isAppearance(v: unknown): v is Appearance {
  return v === "system" || v === "dark" || v === "light";
}

/** 逐字段校验：单个字段损坏（手改 / 旧版本残留）只丢那一个，不整体回默认。 */
function load(): Settings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return { ...DEFAULTS };
    const p = parsed as Record<string, unknown>;
    return {
      scanIntervalSecs: isScanInterval(p.scanIntervalSecs)
        ? p.scanIntervalSecs
        : DEFAULTS.scanIntervalSecs,
      autoCheckUpdates:
        typeof p.autoCheckUpdates === "boolean" ? p.autoCheckUpdates : DEFAULTS.autoCheckUpdates,
      appearance: isAppearance(p.appearance) ? p.appearance : DEFAULTS.appearance,
    };
  } catch {
    return { ...DEFAULTS };
  }
}

let current: Settings = load();
const listeners = new Set<() => void>();

/** 设置项 → 实际生效的主题。system 读 webview 的 prefers-color-scheme ——
 *  仅在 system 档才读它：dark/light 档下 set_window_theme 已覆盖了 webview
 *  的取值，读回来只会是我们自己写进去的值。 */
export function resolveTheme(appearance: Appearance): "dark" | "light" {
  if (appearance !== "system") return appearance;
  try {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  } catch {
    // 非 DOM / matchMedia 不可用：回落历史默认外观
    return "dark";
  }
}

/** 换肤两路：CSS 走 <html data-theme>（App.css 的 :root[data-theme="light"] 块），
 *  原生窗口 chrome（标题栏）走 Rust 的 set_window_theme（AppHandle::set_theme）。
 *  invoke 失败静默 —— 与 i18n 的 set_tray_language 同约定：非 Tauri 环境（单测）
 *  或托盘不可用都不该影响主界面。 */
function applyAppearance(appearance: Appearance) {
  try {
    document.documentElement.dataset.theme = resolveTheme(appearance);
  } catch {
    /* 非 DOM 环境忽略 */
  }
  invoke("set_window_theme", { theme: appearance }).catch(() => {});
}

export function updateSettings(patch: Partial<Settings>) {
  const next = { ...current, ...patch };
  // 逐字段比较，不手抄字段清单：清单漏一项的表现是「改了没反应」——
  // 不落盘、不通知订阅者、不报错，tsc 也看不出来（新字段与旧字段的比较无关）。
  // 与 ProcessRow 把手工维护的 rowPropsEqual 换成单个 shared 对象是同一条教训。
  const keys = Object.keys(next) as (keyof Settings)[];
  if (keys.every((k) => next[k] === current[k])) return;
  const appearanceChanged = next.appearance !== current.appearance;
  current = next;
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(current));
  } catch {
    /* 忽略持久化失败：本次会话仍生效 */
  }
  if (appearanceChanged) applyAppearance(current.appearance);
  listeners.forEach((fn) => fn());
}

function getSettings(): Settings {
  return current;
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** 组件内使用：const { appearance, update } = useSettings() */
export function useSettings() {
  const settings = useSyncExternalStore(subscribe, getSettings);
  return { ...settings, update: updateSettings };
}

// 启动即应用外观（与 i18n 尾部的 set_tray_language 同一初始化位置）。
applyAppearance(current.appearance);

// system 档跟随 OS 实时切换。这个监听兼作 dark/light → system 切换的收尾：
// set_window_theme(None) 让 webview 的 prefers-color-scheme 恢复跟随 OS 是异步的，
// 切换瞬间 resolveTheme 可能还读到旧的覆盖值 —— 恢复完成时本监听会再校正一次。
try {
  window.matchMedia("(prefers-color-scheme: light)").addEventListener("change", () => {
    if (current.appearance === "system") applyAppearance("system");
  });
} catch {
  /* matchMedia 不可用（老 webview）：system 档退化为启动时定格，可接受 */
}
