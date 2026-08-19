// trapTab 的直接单测：焦点圈定是弹窗的键盘安全网，而它有一条只在「弹窗里一个
// 可聚焦元素都没有」时才走到的分支 —— UpdateModal 的下载/安装阶段正是那样
// （modal-actions 只在 available/installed/installFailed 三个阶段渲染按钮）。
// 那条分支此前直接 return，于是 Tab 一路走进弹窗背后的列表，可以在一个
// aria-modal="true" 的弹窗后面按到「终止」按钮上（评审发现）。
import { describe, it, expect, vi } from "vite-plus/test";
import { trapTab } from "./ConfirmModal";

type TabEv = Parameters<typeof trapTab>[0];

/** 最小的 React 合成事件替身：trapTab 只用到这四个成员。 */
function tabEvent(container: HTMLElement, key = "Tab", shiftKey = false) {
  return {
    key,
    shiftKey,
    currentTarget: container,
    preventDefault: vi.fn(),
  } as unknown as TabEv & { preventDefault: ReturnType<typeof vi.fn> };
}

function mount(html: string): HTMLElement {
  const el = document.createElement("div");
  el.innerHTML = html;
  document.body.appendChild(el);
  return el;
}

describe("trapTab 焦点圈定", () => {
  it("非 Tab 键不干预", () => {
    const ev = tabEvent(mount("<button>a</button>"), "Enter");
    trapTab(ev);
    expect(ev.preventDefault).not.toHaveBeenCalled();
  });

  it("焦点在最后一个按钮上时回卷到第一个", () => {
    const el = mount("<button id='a'>a</button><button id='b'>b</button>");
    el.querySelector<HTMLButtonElement>("#b")!.focus();
    const ev = tabEvent(el);
    trapTab(ev);
    expect(ev.preventDefault).toHaveBeenCalled();
    expect(document.activeElement?.id).toBe("a");
  });

  it("没有任何可聚焦元素时吃掉 Tab —— 否则焦点会走进弹窗背后的列表", () => {
    // UpdateModal 下载/安装阶段的形态：一个按钮都没有
    const ev = tabEvent(mount("<div>下载中…</div>"));
    trapTab(ev);
    expect(ev.preventDefault).toHaveBeenCalled();
  });

  it("disabled 按钮不算边界 —— querySelectorAll 找得到，但 Tab 不会停在它上面", () => {
    const el = mount("<button disabled>安装中…</button>");
    const ev = tabEvent(el);
    trapTab(ev);
    expect(ev.preventDefault).toHaveBeenCalled();
    expect(document.activeElement).not.toBe(el.querySelector("button"));
  });
});
