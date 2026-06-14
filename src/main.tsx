import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initFrontendLogging } from "./logger";

// 安装前端日志桥（仅 Tauri webview 内生效），转发未捕获异常到分环境日志文件。
initFrontendLogging();

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
