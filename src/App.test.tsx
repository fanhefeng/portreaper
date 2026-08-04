// App 错误通道回归测试（评审发现的高危 UX bug）：
// 后台 2s 轮询成功后曾无条件 setError(null)，把 kill / 清扫 / 收藏的失败
// 提示在用户看清之前静默冲掉。修复后错误分两路：
//   - scanError：扫描错误，下一次成功轮询自动清除（自愈语义）；
//   - actionError：操作错误，只能用户点击关闭。
// 本文件用假 invoke + 假定时器完整走一遍「kill 失败 → 多轮成功轮询」流程。
import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({
  openUrl: vi.fn(() => Promise.resolve()),
}));

import { invoke } from "@tauri-apps/api/core";
import { ACTION_TIMEOUT_MS, SCAN_TIMEOUT_MS } from "./model";
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
    cpu_percent_tree: 0,
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

  it("kill 挂起不再永久卡死清扫 UI：超时后释放按钮并报错（回归：mutation invoke 曾无超时）", async () => {
    route({
      get_platform: () => "macos",
      scan_ports: () => [suspectEntry()],
      kill_process: () => new Promise(() => {}), // 后端挂死：invoke 永不 settle
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getByText(/Clean up \(1\)/));
    fireEvent.click(screen.getByText("Terminate all"));

    // 超时前：清扫进行中，按钮切到进行态
    await advance(1000);
    expect(screen.getByText(/Cleaning…/)).toBeTruthy();

    // ACTION_TIMEOUT_MS + 700ms 收尾 + freshScan 落定：sweeping 释放、
    // 聚合横幅出现且含本地化的超时文案 —— 修复前这里会永远停在 Cleaning…
    await advance(ACTION_TIMEOUT_MS + 1000);
    const banner = screen.getByText(/1\/1 processes failed/);
    expect(banner.textContent).toContain("timed out");
    const sweepBtn = screen.getByText<HTMLButtonElement>(/Clean up \(1\)/);
    expect(sweepBtn.disabled).toBe(false);
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

  it("白名单键：裸解释器名回退完整命令行（不塌缩匹配全机同名进程）", async () => {
    const added: string[] = [];
    route({
      get_platform: () => "macos",
      // exe_path 是裸名 "node"（PATH/shebang 启动），不含路径分隔符
      scan_ports: () => [
        suspectEntry({
          exe_path: "node",
          full_command: "node /Users/x/proj/server.js",
        }),
      ],
      add_whitelist: (args) => {
        added.push((args as { key: string }).key);
      },
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getByText("☆")); // 收藏
    await advance(0);

    // 键必须是完整命令行，而非塌缩的裸 "node"（前后端镜像一致性）
    expect(added).toEqual(["node /Users/x/proj/server.js"]);
  });

  it("收藏切换后触发全新扫描，而非复用读到落盘前数据的在途扫描（回归：星标曾滞后 2s）", async () => {
    let whitelisted = false;
    let gateNextScan = false;
    const gate: { open: (() => void) | null } = { open: null };
    route({
      get_platform: () => "macos",
      scan_ports: () => {
        // 后端快照语义：扫描开始那一刻读取白名单状态
        const snap = suspectEntry({
          is_whitelisted: whitelisted,
          is_zombie_suspect: !whitelisted,
        });
        if (gateNextScan) {
          gateNextScan = false;
          return new Promise((resolve) => {
            gate.open = () => resolve([snap]);
          });
        }
        return [snap];
      },
      add_whitelist: () => {
        whitelisted = true;
      },
    });
    render(<App />);
    await advance(0); // 首次扫描落定，行未收藏（☆）

    // 下一轮 2s 轮询被门控挂起：它的快照取自白名单写入**之前**
    gateNextScan = true;
    await advance(2000);

    // 用户点星收藏：add_whitelist 落盘成功，但在途扫描还挂着
    fireEvent.click(screen.getByText("☆"));
    await advance(0);

    // 在途的过期扫描落定 —— 修复后 freshScan 会再发起一次全新扫描
    gate.open?.();
    await advance(0);

    // 星标必须立即反映收藏后状态，而不是等下一个 2s 轮询
    expect(screen.getByText("★")).toBeTruthy();
    expect(screen.queryByText("☆")).toBeNull();
  });

  it("终止确认弹窗显示完整命令行（不是 lsof 短名）", async () => {
    route({
      get_platform: () => "macos",
      scan_ports: () => [
        suspectEntry({
          command: "node",
          full_command: "node /Users/x/proj/node_modules/vite/bin/vite.js dev",
        }),
      ],
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getAllByText("Kill")[0]);
    // 弹窗「命令」行展示完整命令，用户能辨认杀的是什么
    expect(screen.getByText("node /Users/x/proj/node_modules/vite/bin/vite.js dev")).toBeTruthy();
  });

  it("清扫进行中禁用行内终止按钮（防对同一进程二次 kill）", async () => {
    // 对象持有器而非裸 let：tsc 的控制流分析看不到闭包内赋值，
    // 会把顶层使用点的 let 收窄为 null（TS2349）
    const killGate: { resolve: (() => void) | null } = { resolve: null };
    route({
      get_platform: () => "macos",
      scan_ports: () => [suspectEntry()],
      kill_process: () =>
        new Promise<void>((r) => {
          killGate.resolve = r;
        }),
    });
    render(<App />);
    await advance(0);

    // 启动清扫（confirmed 1 个）
    fireEvent.click(screen.getByText(/Clean up \(1\)/));
    fireEvent.click(screen.getByText("Terminate all"));
    await advance(0);

    // 清扫期间行内 Kill 按钮被禁用
    const killBtn = screen.getAllByText("Kill")[0] as HTMLButtonElement;
    expect(killBtn.disabled).toBe(true);

    killGate.resolve?.();
    await advance(800);
  });

  it("取消收藏时连 v0.4.0 旧键一并删除（回归：裸键残留导致星标取消不掉）", async () => {
    const removed: string[] = [];
    route({
      get_platform: () => "macos",
      // exe_path 是裸名（PATH/shebang 启动）：新键=完整命令行，旧键=裸 "node"
      scan_ports: () => [
        suspectEntry({
          exe_path: "node",
          full_command: "node /Users/x/proj/server.js",
          is_whitelisted: true,
          is_zombie_suspect: false,
          confidence: "none",
        }),
      ],
      remove_whitelist: (args) => {
        removed.push((args as { key: string }).key);
      },
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getByText("★")); // 取消收藏
    await advance(0);

    // 必须按「新键 → 旧键」顺序各调一次 remove_whitelist
    expect(removed).toEqual(["node /Users/x/proj/server.js", "node"]);
  });

  it("扫描超时转可见错误（自愈语义）：后端恢复后下一轮自动清除", async () => {
    let hang = true;
    route({
      get_platform: () => "macos",
      scan_ports: () => (hang ? new Promise(() => {}) : ([suspectEntry()] as unknown)),
    });
    render(<App />);
    await advance(0);

    // 超时阈值后：挂死的 invoke 被转成本地化的扫描错误横幅
    await advance(SCAN_TIMEOUT_MS + 100);
    expect(screen.getByText(/Scan timed out/)).toBeTruthy();

    // 后端恢复。注意超时那一刻轮询已立即用仍挂死的后端开了第二次扫描 ——
    // 它还要再等一个完整超时周期；其后的下一轮 2s 轮询才能成功并自愈
    hang = false;
    await advance(SCAN_TIMEOUT_MS + 2100);
    expect(screen.queryByText(/Scan timed out/)).toBeNull();
  });

  it("Windows 平台行内是单一 Terminate 按钮（无 SIGTERM/强杀双按钮）", async () => {
    route({
      get_platform: () => "windows",
      scan_ports: () => [suspectEntry()],
    });
    render(<App />);
    await advance(0);

    expect(screen.getAllByText("Terminate").length).toBe(1);
    expect(screen.queryByText("Kill")).toBeNull();
    expect(screen.queryByText("Force")).toBeNull();
  });

  it("并发 kill：先完成的一次不得清掉仍在飞行那次的 killing 标记（评审发现）", async () => {
    const gates: Record<number, () => void> = {};
    route({
      get_platform: () => "macos",
      scan_ports: () => [suspectEntry(), suspectEntry({ pid: 5151, ports: [5174] })],
      kill_process: (args) =>
        new Promise<void>((r) => {
          gates[(args as { pid: number }).pid] = r;
        }),
    });
    render(<App />);
    await advance(0);

    // kill 4242（挂起中）
    fireEvent.click(screen.getAllByText("Kill")[0]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(0);

    // kill 5151（也挂起）—— killingPid 现在是 5151
    fireEvent.click(screen.getAllByText("Kill")[1]);
    fireEvent.click(screen.getByText("Terminate"));
    await advance(0);

    // 4242 先完成：函数式更新只清自己的标记，5151 的按钮必须仍处于禁用
    gates[4242]?.();
    await advance(400);
    const killBtnB = screen.getAllByText("Kill")[1] as HTMLButtonElement;
    expect(killBtnB.disabled).toBe(true);

    gates[5151]?.();
    await advance(400);
  });

  it("「可疑」标签页零嫌疑时显示一切正常，而非「没有匹配项」", async () => {
    route({
      get_platform: () => "macos",
      scan_ports: () => [
        suspectEntry({
          is_zombie_suspect: false,
          confidence: "none",
          zombie_reasons: [],
        }),
      ],
    });
    render(<App />);
    await advance(0);

    fireEvent.click(screen.getByText("Zombies")); // 切到「可疑」tab
    expect(screen.getByText(/All clear/)).toBeTruthy();
    expect(screen.queryByText("No matches")).toBeNull();
  });

  it('端口搜索接受 ":5173" 形式（UI 展示格式可原样复制粘贴）', async () => {
    route({
      get_platform: () => "macos",
      scan_ports: () => [
        suspectEntry(),
        suspectEntry({
          pid: 1111,
          ports: [8080],
          command: "other",
          full_command: "other server",
          app_label: "other",
        }),
      ],
    });
    render(<App />);
    await advance(0);

    fireEvent.change(screen.getByPlaceholderText(/Search/), {
      target: { value: ":5173" },
    });
    // 仅 :5173 的行保留
    expect(screen.getByText("proj")).toBeTruthy();
    expect(screen.queryByText("other")).toBeNull();
  });
});
