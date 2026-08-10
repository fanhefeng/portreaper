import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initFrontendLogging } from "./logger";
import { ErrorBoundary } from "./components/ErrorBoundary";

// 安装前端日志桥（仅 Tauri webview 内生效），转发未捕获异常到分环境日志文件。
initFrontendLogging();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* 渲染期异常此前会卸载整棵树：托盘图标还在、计数还在更新，窗口却是一片
        空白 —— 对一个常驻托盘的工具来说这是最劝退的失败态，而且用户没有任何
        线索可交。ErrorBoundary 把它变成一个带日志入口的可读页面。
        必须在 StrictMode 之内、App 之外：App 自己的 error-region 只覆盖**已预期**
        的扫描/操作失败，覆盖不了 render 本身抛出的异常。 */}
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
