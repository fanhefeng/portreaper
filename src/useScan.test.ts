// useScan 的直接单测：轮询并发语义（inFlight 复用、freshScan「等在途再扫」）
// 此前只能靠 App.test.tsx 整棵渲染 + 假定时器间接覆盖（评审发现）。
// App 级的错误通道展示（自愈 vs 点击关闭的优先级）仍在 App.test.tsx。
import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { act, renderHook } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));

import { invoke } from "@tauri-apps/api/core";
import { makeEntry } from "./test-fixtures";
import { useScan } from "./useScan";

const mockInvoke = vi.mocked(invoke);

/** 按命令名路由的假 invoke（与 App.test.tsx 同一约定）；未注册命令静默成功 */
function route(handlers: Record<string, (args?: unknown) => unknown>) {
  mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
    const h = handlers[cmd];
    if (!h) return Promise.resolve(undefined);
    return Promise.resolve().then(() => h(args)) as Promise<never>;
  });
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
}

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
  mockInvoke.mockReset();
  mockInvoke.mockImplementation(() => Promise.resolve(undefined));
});

describe("useScan 对外契约（终止后存活确认依赖它）", () => {
  it("freshScan 返回本轮 entries —— 只写 state 的话调用方拿不到与本次动作对应的快照", async () => {
    route({ scan_ports: () => [makeEntry({ pid: 777 })] });
    const { result } = renderHook(() => useScan());
    await advance(0);

    let rows: unknown = "unset";
    await act(async () => {
      rows = await result.current.freshScan();
    });
    expect(Array.isArray(rows)).toBe(true);
    expect((rows as { pid: number }[])[0].pid).toBe(777);
  });

  it("扫描失败时 freshScan 返回 null（= 没有证据），不是空数组（= 什么都不剩）", async () => {
    route({
      scan_ports: () => {
        throw "boom";
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);

    let rows: unknown = "unset";
    await act(async () => {
      rows = await result.current.freshScan();
    });
    // 这个区分是存活确认的安全阀：空数组会被读成「目标已消失」
    expect(rows).toBeNull();
  });

  it("lastScanOk 独立于可点掉的 scanError —— 清掉横幅不该让空态退化成「一切正常」", async () => {
    route({
      scan_ports: () => {
        throw "boom";
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);

    expect(result.current.hasScanned).toBe(true);
    expect(result.current.lastScanOk).toBe(false);
    act(() => result.current.clearScanError());
    expect(result.current.scanError).toBeNull();
    expect(result.current.lastScanOk).toBe(false);
  });

  it("hasScanned 在首轮落定前为假 —— 否则启动首帧会宣布「没有发现任何监听端口」", async () => {
    route({ scan_ports: () => [makeEntry()] });
    const { result } = renderHook(() => useScan());
    expect(result.current.hasScanned).toBe(false);
    await advance(0);
    expect(result.current.hasScanned).toBe(true);
  });
});

describe("useScan polling", () => {
  it("挂载即扫描一次，此后每 2s 轮询", async () => {
    let scans = 0;
    route({
      scan_ports: () => {
        scans += 1;
        return [makeEntry()];
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);
    expect(scans).toBe(1);
    expect(result.current.entries.length).toBe(1);

    await advance(2000);
    expect(scans).toBe(2);
    await advance(2000);
    expect(scans).toBe(3);
  });

  it("轮询间隔可注入（设置的扫描间隔档）：5s 档下 2s 不触发、满 5s 触发", async () => {
    let scans = 0;
    route({
      scan_ports: () => {
        scans += 1;
        return [makeEntry()];
      },
    });
    renderHook(() => useScan(5000));
    await advance(0);
    expect(scans).toBe(1);

    await advance(2100);
    expect(scans).toBe(1); // 缺省 2s 档会在这里触发 —— 注入值必须真的生效
    // 卡在 5s 边界两侧断言：只断言「2.1s 不触发、5.1s 触发」的话，注入值退化成
    // 3s/4s 档同样能过 —— 那正是这条测试要盯住的漂移（评审发现）。
    await advance(2899);
    expect(scans).toBe(1);
    await advance(1);
    expect(scans).toBe(2);
  });

  it("inFlight 复用：上一轮扫描未落定时，后续轮询不发起并发扫描", async () => {
    let scans = 0;
    const gate: { open: (() => void) | null } = { open: null };
    route({
      scan_ports: () => {
        scans += 1;
        return new Promise((resolve) => {
          gate.open = () => resolve([makeEntry()]);
        });
      },
    });
    renderHook(() => useScan());
    await advance(0);
    expect(scans).toBe(1);

    // 两个轮询周期过去，第一次扫描仍挂着 —— 绝不能叠加新扫描
    await advance(4100);
    expect(scans).toBe(1);

    // 扫描落定后，下一轮轮询恢复正常
    gate.open?.();
    await advance(2100);
    expect(scans).toBe(2);
  });

  it("freshScan 等掉在途扫描后再扫一次（拿到变更后的真实状态）", async () => {
    const seen: number[] = [];
    let round = 0;
    const gate: { open: (() => void) | null } = { open: null };
    route({
      scan_ports: () => {
        round += 1;
        seen.push(round);
        if (round === 1) {
          // 第一轮挂起：模拟 freshScan 调用时正有过期扫描在飞行中
          return new Promise((resolve) => {
            gate.open = () => resolve([makeEntry({ pid: 1 })]);
          });
        }
        return [makeEntry({ pid: 100 + round })];
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);
    expect(seen).toEqual([1]);

    // 在途扫描挂着时调 freshScan：它必须等第一轮收尾，然后自己再扫一轮
    let settled = false;
    let freshPromise: Promise<void> = Promise.resolve();
    act(() => {
      freshPromise = result.current.freshScan().then(() => {
        settled = true;
      });
    });
    await advance(0);
    expect(settled).toBe(false); // 还在等在途扫描
    expect(seen).toEqual([1]); // 第二轮尚未发起

    gate.open?.();
    await advance(0);
    await act(async () => {
      await freshPromise;
    });
    expect(settled).toBe(true);
    expect(seen).toEqual([1, 2]); // 等完在途后确实又扫了一轮
    // entries 是 freshScan 那一轮（round 2）的结果，不是过期的第一轮
    expect(result.current.entries[0]?.pid).toBe(102);
  });

  it("扫描失败设 scanError，下一次成功轮询自动清除（自愈语义）", async () => {
    let fails = true;
    route({
      scan_ports: () => {
        if (fails) throw "lsof exploded";
        return [makeEntry()];
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);
    expect(result.current.scanError).toContain("lsof exploded");

    fails = false;
    await advance(2100);
    expect(result.current.scanError).toBeNull();
  });

  it("托盘推送失败不把成功的扫描标成错误（fire-and-forget，评审发现）", async () => {
    route({
      scan_ports: () => [makeEntry()],
      update_tray_title: () => {
        throw "tray unavailable";
      },
    });
    const { result } = renderHook(() => useScan());
    await advance(0);
    expect(result.current.entries.length).toBe(1);
    expect(result.current.scanError).toBeNull();
  });

  it("托盘计数只含可清扫层级（Confirmed+Likely），端口计数为全部端口和", async () => {
    const trayCalls: Array<{ count: number; suspectCount: number }> = [];
    route({
      scan_ports: () => [
        makeEntry(), // confirmed
        makeEntry({ pid: 2, ports: [5174], confidence: "likely" }),
        makeEntry({ pid: 3, ports: [5175, 5176], confidence: "possible" }),
      ],
      update_tray_title: (args) => {
        trayCalls.push(args as { count: number; suspectCount: number });
      },
    });
    renderHook(() => useScan());
    await advance(0);
    expect(trayCalls).toEqual([{ count: 4, suspectCount: 2 }]);
  });

  it("卸载后停止轮询", async () => {
    let scans = 0;
    route({
      scan_ports: () => {
        scans += 1;
        return [];
      },
    });
    const { unmount } = renderHook(() => useScan());
    await advance(0);
    expect(scans).toBe(1);
    unmount();
    await advance(6000);
    expect(scans).toBe(1);
  });
});
