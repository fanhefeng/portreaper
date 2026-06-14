import { error, info } from "@tauri-apps/plugin-log";

/**
 * 前端日志桥：把 webview 里的未捕获异常转发到后端 tauri-plugin-log，
 * 与 Rust 日志写进同一份分环境日志文件（dev 与 prod 互不混淆，见 src-tauri/src/paths.rs）。
 *
 * macOS/Windows 的正式版都是 GUI 进程、无开发者工具常驻，前端崩溃在生产环境
 * 本来无迹可寻 —— 这里补上「未捕获错误 / 未处理 Promise 拒绝」两条最关键的线索。
 *
 * 仅在 Tauri webview 内生效：vitest（happy-dom）与纯浏览器预览没有
 * __TAURI_INTERNALS__，此时静默跳过，绝不污染测试，也不会因 invoke 失败报错。
 */
export function initFrontendLogging(): void {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return;
  }

  void info("webview ready");

  window.addEventListener("error", (e: ErrorEvent) => {
    const where = `${e.filename}:${e.lineno}:${e.colno}`;
    // e.error 是真正的 Error 对象(带 stack);e.message 只是摘要。生产 GUI 版
    // 无常驻开发者工具,stack 是唯一可追因的线索 —— 有 Error 就优先记 stack。
    const detail =
      e.error instanceof Error ? (e.error.stack ?? e.error.message) : e.message;
    void error(`uncaught error: ${detail} @ ${where}`);
  });

  window.addEventListener("unhandledrejection", (e: PromiseRejectionEvent) => {
    const r: unknown = e.reason;
    // String(reason) 对 Error 只给 "Error: msg"(丢 stack),对普通对象给
    // "[object Object]" —— 两者都让生产崩溃无从追起。是 Error 就取 stack。
    const detail =
      r instanceof Error ? (r.stack ?? `${r.name}: ${r.message}`) : String(r);
    void error(`unhandled rejection: ${detail}`);
  });
}
