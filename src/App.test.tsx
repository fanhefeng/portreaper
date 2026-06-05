// App 错误通道回归测试（评审发现的高危 UX bug）：
// 后台 2s 轮询成功后曾无条件 setError(null)，把 kill / 清扫 / 收藏的失败
// 提示在用户看清之前静默冲掉。修复后错误分两路：
//   - scanError：扫描错误，下一次成功轮询自动清除（自愈语义）；
//   - actionError：操作错误，只能用户点击关闭。
// 本文件用假 invoke + 假定时器完整走一遍「kill 失败 → 多轮成功轮询」流程。
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

import { invoke } from "@tauri-apps/api/core";
import App from "./App";

const mockInvoke = vi.mocked(invoke);

/** 一行可被 kill 的 Confirmed 嫌疑（serde 契约的最小镜像），字段可覆盖 */
function suspectEntry(over: Record<string, unknown> = {}) {
  return {
    pid: 4242,
    ppid: 1,
    ports: [5173],
    command: "node",
    full_command: "node /Users/x/proj/node_modules/vite/bin/vite.js",
    exe_path: "/opt/homebrew/bin/node",
    app_label: "proj · vite.js",
    app_category: "dev-script",
    parent_chain: [],
    launcher_label: "launchd",
    user: "x",
    tty: "",
    elapsed_secs: 3600,
    start_unix: 1000,
    cpu_percent: 0,
    mem_mb: 10,
    state: "S",
    is_zombie_suspect: true,
    confidence: "confirmed",
    zombie_reasons: ["ppid1_orphan", "dev_server_keyword"],
    is_whitelisted: false,
    duplicate_of: null,
    ...over,
  };
}

/** 按命令名路由的假 invoke；未注册的命令静默成功（update_tray_title 等装饰性调用） */
function route(handlers: Record<string, (args?: unknown) => unknown>) {
  mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
    const h = handlers[cmd];
    if (!h) return Promise.resolve(undefined);
    return Promise.resolve().then(() => h(args)) as Promise<never>;
  });
}

/** 推进假定时器并清空微任务队列（让 in-flight 的 invoke promise 落定） */
async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

describe("error channels", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    // 语言固定走英文分支，断言文案不受系统 locale 影响
    localStorage.setItem("portreaper.lang", "en");
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
    mockInvoke.mockReset();
    mockInvoke.mockImplementation(() => Promise.resolve(undefined));
    localStorage.clear();
  });

  it("kill 失败的操作错误在后续成功轮询后仍然可见（回归：曾被 2s 轮询冲掉）", async () => {
    route({
      get_platform: () => "macos",
      scan_ports: () => [suspectEntry()],
      kill_process: () => {
        throw "ERR_PID_REUSED: process identity changed";
      },
    });
    render(<App />);
    await advance(0); // 首次扫描落定

    // 行内 Kill → 确认弹窗 → Terminate
    fireEvent.click(screen.getAllByText("Kill")[0]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(0); // kill 拒绝落定

    // 错误出现，且是本地化后的语义文案
    expect(screen.getByText(/Kill failed/)).toBeTruthy();
    expect(screen.getByText(/PID was reused/)).toBeTruthy();

    // 两轮成功轮询（>4s）之后错误必须仍在 —— 这就是被修复的回归
    await advance(4500);
    expect(screen.getByText(/Kill failed/)).toBeTruthy();

    // 用户点击后才消失
    fireEvent.click(screen.getByText(/Kill failed/));
    expect(screen.queryByText(/Kill failed/)).toBeNull();
  });

  it("后续操作成功后清除残留失败横幅：横幅反映最近一次操作的结果", async () => {
    let killFails = true;
    route({
      get_platform: () => "macos",
      scan_ports: () => [suspectEntry()],
      kill_process: () => {
        if (killFails) throw "ERR_PROCESS_GONE: process no longer exists";
        return undefined;
      },
    });
    render(<App />);
    await advance(0);

    // 第一次 kill 失败 → 横幅出现
    fireEvent.click(screen.getAllByText("Kill")[0]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(0);
    expect(screen.getByText(/Kill failed/)).toBeTruthy();

    // 第二次 kill 成功 → 残留失败横幅被清除（无需用户点击）
    killFails = false;
    fireEvent.click(screen.getAllByText("Kill")[0]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(400); // 成功路径有 250ms 收尾等待 + freshScan
    expect(screen.queryByText(/Kill failed/)).toBeNull();
  });

  it("批量清扫只含 Confirmed+Likely，失败聚合横幅在轮询后仍可见", async () => {
    const killCalls: number[] = [];
    route({
      get_platform: () => "macos",
      scan_ports: () => [
        suspectEntry(), // confirmed, pid 4242
        suspectEntry({ pid: 4343, confidence: "likely", ports: [5174] }),
        suspectEntry({ pid: 4444, confidence: "possible", ports: [5175] }),
      ],
      kill_process: (args) => {
        killCalls.push((args as { pid: number }).pid);
        throw "ERR_PID_REUSED: process identity changed";
      },
    });
    render(<App />);
    await advance(0);

    // 清扫按钮计数 = 2（Possible 永不入清扫 —— CLAUDE.md 不变量）
    fireEvent.click(screen.getByText(/Clean up \(2\)/));
    fireEvent.click(screen.getByText("Terminate all"));
    await advance(800); // 批量循环 + 700ms 收尾等待

    // 清扫范围：只杀了 confirmed + likely，4444 从未被尝试
    expect(killCalls.sort()).toEqual([4242, 4343]);
    expect(killCalls).not.toContain(4444);

    // 失败聚合横幅：2/2 + 语言无关分隔符「; 」
    const banner = screen.getByText(/2\/2 processes failed/);
    expect(banner.textContent).toContain("; PID 4343");

    // 两轮成功轮询后仍可见（actionError 不被轮询冲掉）
    await advance(4500);
    expect(screen.getByText(/2\/2 processes failed/)).toBeTruthy();
  });

  it("扫描错误保留自愈语义：后端恢复后下一轮轮询自动清除", async () => {
    let scanFails = true;
    route({
      get_platform: () => "macos",
      scan_ports: () => {
        if (scanFails) throw "lsof exploded";
        return [suspectEntry()];
      },
    });
    render(<App />);
    await advance(0);
    expect(screen.getByText(/lsof exploded/)).toBeTruthy();

    scanFails = false;
    await advance(2100); // 下一轮轮询成功
    expect(screen.queryByText(/lsof exploded/)).toBeNull();
  });

  it("操作错误优先于扫描错误展示，关闭时两者同时清空", async () => {
    let scanFails = false;
    route({
      get_platform: () => "macos",
      scan_ports: () => {
        if (scanFails) throw "scan down";
        return [suspectEntry()];
      },
      kill_process: () => {
        throw "ERR_PROCESS_GONE: process no longer exists";
      },
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getAllByText("Kill")[0]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(0);

    // 扫描也开始失败：展示的仍是操作错误（actionError ?? scanError）
    scanFails = true;
    await advance(2100);
    expect(screen.getByText(/Kill failed/)).toBeTruthy();
    expect(screen.queryByText(/scan down/)).toBeNull();

    // 点击关闭后，扫描错误在下一轮失败时可以重新出现（通道未被永久压制）
    fireEvent.click(screen.getByText(/Kill failed/));
    await advance(2100);
    expect(screen.getByText(/scan down/)).toBeTruthy();
  });
});
