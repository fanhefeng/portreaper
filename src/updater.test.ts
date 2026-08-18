// useUpdater 的直接单测：状态机转换（检查 → available / upToDate / checkFailed、
// 安装 → 进度 → installed / installFailed）与「dev 下不自动检查」的门。
// 进度经命令参数里的 Channel 回传 —— mock 一个只有 onmessage 的壳即可。
import { describe, it, expect, vi, beforeEach, afterEach } from "vite-plus/test";
import { act, renderHook } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => {
  class MockChannel<T> {
    onmessage: (msg: T) => void = () => {};
  }
  return { invoke: vi.fn(() => Promise.resolve(undefined)), Channel: MockChannel };
});

import { invoke } from "@tauri-apps/api/core";
import { TRANSIENT_REVERT_MS, useUpdater, type InstallProgress, type UpdateInfo } from "./updater";

const mockInvoke = vi.mocked(invoke);

const INFO: UpdateInfo = { version: "9.9.9", current_version: "0.0.1", notes: "notes" };

/** 按命令名路由的假 invoke（与 useScan.test.ts 同一约定）；未注册命令静默成功 */
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

describe("useUpdater 检查一路", () => {
  it("dev 环境下挂载不自动检查（vitest 里 import.meta.env.DEV 为 true）", async () => {
    renderHook(() => useUpdater());
    await advance(0);
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it("手动检查发现新版本 → available + 弹窗打开", async () => {
    route({ check_update: () => INFO });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(true);
    });
    expect(result.current.state).toMatchObject({ phase: "available", info: INFO });
    expect(result.current.modalOpen).toBe(true);
  });

  it("手动检查无更新 → upToDate，短暂停留后自动回落 idle", async () => {
    route({ check_update: () => null });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(true);
    });
    expect(result.current.state.phase).toBe("upToDate");
    await advance(TRANSIENT_REVERT_MS);
    expect(result.current.state.phase).toBe("idle");
  });

  it("手动检查失败 → checkFailed 且保留原始错误文本（报 issue 的线索）", async () => {
    route({
      check_update: () => {
        throw "endpoint 404";
      },
    });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(true);
    });
    expect(result.current.state).toMatchObject({ phase: "checkFailed", message: "endpoint 404" });
  });

  it("自动检查失败保持静默（latest.json 未上线前 404 是常态）→ 回 idle", async () => {
    route({
      check_update: () => {
        throw "endpoint 404";
      },
    });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(false);
    });
    expect(result.current.state.phase).toBe("idle");
  });
});

describe("useUpdater 安装一路", () => {
  it("available → downloading（吃进度）→ installing → installed", async () => {
    let emit: ((msg: InstallProgress) => void) | null = null;
    let finish: (() => void) | null = null;
    route({
      check_update: () => INFO,
      install_update: (args) => {
        const ch = (args as { onProgress: { onmessage: (msg: InstallProgress) => void } })
          .onProgress;
        emit = (msg) => ch.onmessage(msg);
        return new Promise<void>((resolve) => {
          finish = resolve;
        });
      },
    });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(true);
    });

    // install 在返回的 Promise 挂起期间就要能看到中间态，不 await 它
    let installDone: Promise<void> | null = null;
    await act(async () => {
      installDone = result.current.install();
      await vi.advanceTimersByTimeAsync(0);
    });
    expect(result.current.state.phase).toBe("downloading");

    await act(async () => {
      emit!({ event: "chunk", downloaded: 512, total: 1024 });
    });
    expect(result.current.state).toMatchObject({
      phase: "downloading",
      downloaded: 512,
      total: 1024,
    });

    await act(async () => {
      emit!({ event: "installing" });
    });
    expect(result.current.state.phase).toBe("installing");

    await act(async () => {
      finish!();
      await installDone;
    });
    expect(result.current.state).toMatchObject({ phase: "installed", info: INFO });
  });

  it("安装失败 → installFailed（保留 info，重试无需重新 check）", async () => {
    route({
      check_update: () => INFO,
      install_update: () => {
        throw "disk full";
      },
    });
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.check(true);
    });
    await act(async () => {
      await result.current.install();
    });
    expect(result.current.state).toMatchObject({
      phase: "installFailed",
      info: INFO,
      message: "disk full",
    });
  });

  it("未处于 available/installFailed 时 install 是空操作（并发兜底）", async () => {
    const { result } = renderHook(() => useUpdater());
    await act(async () => {
      await result.current.install();
    });
    expect(result.current.state.phase).toBe("idle");
    expect(mockInvoke).not.toHaveBeenCalledWith("install_update", expect.anything());
  });
});
