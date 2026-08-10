import React from "react";
import { invoke } from "@tauri-apps/api/core";
import { logRenderCrash } from "../logger";

type Props = { children: React.ReactNode };
type State = { error: Error | null; copied: "ok" | "failed" | null };

/**
 * 渲染期异常的兜底页。
 *
 * 没有它的话，任何 render 抛错（某个契约字段形状变了、`describe.ts` 拿到意料外的
 * 输入……）都会卸载整棵 React 树 —— 而托盘图标还在、计数还在更新，用户看到的是
 * 一个空白窗口和零线索。App 自己的 error-region 覆盖不了这一层：它只处理**已预期**
 * 的扫描 / 操作失败。
 *
 * React 19 仍然只有 class 组件能做错误边界（没有 hook 版），故这里刻意保留 class，
 * 而不是为此引入一个第三方依赖。
 *
 * **诊断文本里不放任何进程信息**（命令行、cwd、项目路径都可能含 token 或私有路径）——
 * 只有版本、平台与错误本身。这与 README 承诺的「不上报任何进程信息」同向：日志虽然
 * 只在本机，但它正是用户会直接贴进 issue 的东西。
 */
export class ErrorBoundary extends React.Component<Props, State> {
  state: State = { error: null, copied: null };

  static getDerivedStateFromError(error: Error): State {
    return { error, copied: null };
  }

  componentDidCatch(error: Error, info: React.ErrorInfo) {
    // 走与 window.onerror 同一条桥（内部已吞掉自身失败，不会自激）
    logRenderCrash(error, info.componentStack ?? "");
  }

  /** 复制诊断信息。**失败必须可见**：这一页唯一的职责就是帮用户把线索交出去，
   *  一个静默失效的复制按钮等于把它变成一页死文本。clipboard 不可用时把全文
   *  摊在页面上（`.crash-detail` 是可选中的），用户至少能手动复制。 */
  private copyDiagnostics(text: string) {
    const fail = () => this.setState({ copied: "failed" });
    try {
      const p = navigator.clipboard?.writeText(text);
      if (!p) return fail();
      void p.then(() => this.setState({ copied: "ok" }), fail);
    } catch {
      fail();
    }
  }

  render() {
    const { error, copied } = this.state;
    if (!error) return this.props.children;

    const diagnostics = [
      `Portreaper ${__APP_VERSION__}`,
      `UA: ${navigator.userAgent}`,
      `Error: ${error.name}: ${error.message}`,
      error.stack ?? "",
    ].join("\n");

    return (
      <div className="crash" role="alert">
        <h1>Portreaper hit a rendering error</h1>
        <p>
          界面渲染时抛出了异常，本次会话的窗口无法继续。托盘图标仍在运行 ——
          重新打开窗口通常就能恢复。
          <br />
          The window crashed while rendering. The tray is still running; reopening the window
          usually recovers.
        </p>
        <pre className="crash-detail">
          {copied === "failed" ? diagnostics : `${error.name}: ${error.message}`}
        </pre>
        <div className="crash-actions">
          <button className="btn-ghost" onClick={() => this.copyDiagnostics(diagnostics)}>
            {copied === "ok"
              ? "已复制 / Copied"
              : copied === "failed"
                ? "复制失败，请手动选中上方文本 / Copy failed — select the text above"
                : "复制诊断信息 / Copy diagnostics"}
          </button>
          <button
            className="btn-ghost"
            onClick={() => {
              // 与托盘菜单「打开日志目录」同一个后端命令，不新增权限
              void invoke("open_log_dir").catch(() => {});
            }}
          >
            打开日志目录 / Open log folder
          </button>
          <button
            className="btn-ghost"
            onClick={() => this.setState({ error: null, copied: null })}
          >
            重试 / Retry
          </button>
        </div>
      </div>
    );
  }
}
