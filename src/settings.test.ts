// settings 模块单测：localStorage 持久化、逐字段校验回退、订阅通知、
// 外观应用（data-theme + set_window_theme 同步）。
// 模块有 import 时副作用（load + applyAppearance），需要干净初始状态的用例
// 走 vi.resetModules + 动态 import —— mock 工厂随之重建，invoke 实例要一并重取。
import { describe, it, expect, vi, afterEach } from "vite-plus/test";
import { act, renderHook } from "@testing-library/react";

vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => Promise.resolve(undefined)),
}));

type SettingsModule = typeof import("./settings");

/** 重置模块注册表后按当前 localStorage 重新求值 settings 模块，
 *  连同它实际引用的那份 invoke mock 一起返回。 */
async function freshSettings() {
  vi.resetModules();
  const core = await import("@tauri-apps/api/core");
  const mod: SettingsModule = await import("./settings");
  return { mod, invoke: vi.mocked(core.invoke) };
}

afterEach(() => {
  localStorage.clear();
  vi.unstubAllGlobals();
});

describe("settings 持久化", () => {
  it("未存储时给缺省值；appearance 缺省 dark（升级用户不换肤）", async () => {
    localStorage.clear();
    const { mod } = await freshSettings();
    const { result } = renderHook(() => mod.useSettings());
    expect(result.current.scanIntervalSecs).toBe(2);
    expect(result.current.autoCheckUpdates).toBe(true);
    expect(result.current.appearance).toBe("dark");
    // import 时即应用外观：首帧就有 data-theme，不等 React 挂载
    expect(document.documentElement.dataset.theme).toBe("dark");
  });

  it("逐字段校验：单个字段损坏只丢那一个，不整体回默认", async () => {
    localStorage.setItem(
      "portreaper.settings",
      // scanIntervalSecs 7 不在档位表、autoCheckUpdates 是字符串 —— 都该回退
      JSON.stringify({ scanIntervalSecs: 7, autoCheckUpdates: "yes", appearance: "light" }),
    );
    const { mod } = await freshSettings();
    const { result } = renderHook(() => mod.useSettings());
    expect(result.current.scanIntervalSecs).toBe(2);
    expect(result.current.autoCheckUpdates).toBe(true);
    expect(result.current.appearance).toBe("light");
  });

  it("存储不是 JSON 时整体回默认（不抛）", async () => {
    localStorage.setItem("portreaper.settings", "not-json{");
    const { mod } = await freshSettings();
    const { result } = renderHook(() => mod.useSettings());
    expect(result.current.scanIntervalSecs).toBe(2);
  });

  it("updateSettings 落盘并通知订阅者", async () => {
    localStorage.clear();
    const { mod } = await freshSettings();
    const { result } = renderHook(() => mod.useSettings());
    act(() => mod.updateSettings({ scanIntervalSecs: 5, autoCheckUpdates: false }));
    expect(result.current.scanIntervalSecs).toBe(5);
    expect(result.current.autoCheckUpdates).toBe(false);
    const stored = JSON.parse(localStorage.getItem("portreaper.settings") ?? "{}") as Record<
      string,
      unknown
    >;
    expect(stored.scanIntervalSecs).toBe(5);
    expect(stored.autoCheckUpdates).toBe(false);
  });
});

describe("外观", () => {
  it("切换 appearance 写 <html data-theme> 并同步原生主题（set_window_theme）", async () => {
    localStorage.clear();
    const { mod, invoke } = await freshSettings();
    invoke.mockClear(); // 丢掉 import 时那次初始应用
    act(() => mod.updateSettings({ appearance: "light" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(invoke).toHaveBeenCalledWith("set_window_theme", { theme: "light" });
  });

  it("非外观字段的变更不触发原生主题同步", async () => {
    localStorage.clear();
    const { mod, invoke } = await freshSettings();
    invoke.mockClear();
    act(() => mod.updateSettings({ scanIntervalSecs: 3 }));
    expect(invoke).not.toHaveBeenCalledWith("set_window_theme", expect.anything());
  });

  it("resolveTheme：dark/light 直通，system 读 prefers-color-scheme", async () => {
    const { mod } = await freshSettings();
    expect(mod.resolveTheme("dark")).toBe("dark");
    expect(mod.resolveTheme("light")).toBe("light");

    const fakeMatchMedia = (query: string) =>
      ({
        matches: query.includes("light"),
        addEventListener: () => {},
      }) as unknown as MediaQueryList;
    vi.stubGlobal("matchMedia", fakeMatchMedia);
    expect(mod.resolveTheme("system")).toBe("light");
  });

  it("system 档端到端：data-theme 落解析后的值，而原生侧仍收到 'system'", async () => {
    // 两侧刻意不同值：CSS 需要一个具体主题，原生侧要的是「跟随 OS」（None）。
    // 只测 resolveTheme 覆盖不到这条 —— 它是 applyAppearance 里的分工。
    vi.stubGlobal(
      "matchMedia",
      (query: string) =>
        ({
          matches: query.includes("light"),
          addEventListener: () => {},
        }) as unknown as MediaQueryList,
    );
    localStorage.clear();
    const { mod, invoke } = await freshSettings();
    invoke.mockClear();
    act(() => mod.updateSettings({ appearance: "system" }));
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(invoke).toHaveBeenCalledWith("set_window_theme", { theme: "system" });
  });

  it("OS 主题变化时 system 档跟着走，dark/light 档不受影响", async () => {
    // 模块级的 matchMedia change 监听（settings.ts 尾部）。注释说它兼任
    // 「dark/light → system 切换的收尾校正」，是这套外观逻辑里唯一依赖异步事件
    // 才能得到正确结果的地方 —— 此前完全没有覆盖（评审发现）。
    const listeners: Array<() => void> = [];
    let prefersLight = false;
    vi.stubGlobal(
      "matchMedia",
      (query: string) =>
        ({
          get matches() {
            return query.includes("light") ? prefersLight : !prefersLight;
          },
          addEventListener: (_: string, fn: () => void) => listeners.push(fn),
        }) as unknown as MediaQueryList,
    );
    localStorage.clear();
    const { mod } = await freshSettings();
    expect(listeners.length).toBe(1);

    act(() => mod.updateSettings({ appearance: "system" }));
    expect(document.documentElement.dataset.theme).toBe("dark");

    // OS 切到浅色 → 监听器把 data-theme 校正过来
    prefersLight = true;
    act(() => listeners.forEach((fn) => fn()));
    expect(document.documentElement.dataset.theme).toBe("light");

    // 固定档不该被 OS 变化带走
    act(() => mod.updateSettings({ appearance: "dark" }));
    prefersLight = false;
    act(() => listeners.forEach((fn) => fn()));
    expect(document.documentElement.dataset.theme).toBe("dark");
  });
});

describe("降级路径", () => {
  it("localStorage 写入抛异常时本次会话仍生效，不冒泡给调用方", async () => {
    // 隐私模式 / 配额耗尽：setItem 会抛。设置是纯偏好，存不下来也不该中断操作。
    localStorage.clear();
    const { mod } = await freshSettings();
    const setItem = vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new DOMException("QuotaExceededError");
    });
    try {
      const { result } = renderHook(() => mod.useSettings());
      expect(() => act(() => mod.updateSettings({ scanIntervalSecs: 10 }))).not.toThrow();
      expect(result.current.scanIntervalSecs).toBe(10); // 内存里生效
    } finally {
      setItem.mockRestore();
    }
  });

  it("matchMedia 不可用（老 webview）时 import 不抛，system 档退化为 dark", async () => {
    vi.stubGlobal("matchMedia", () => {
      throw new Error("not supported");
    });
    localStorage.setItem("portreaper.settings", JSON.stringify({ appearance: "system" }));
    const { mod } = await freshSettings();
    expect(mod.resolveTheme("system")).toBe("dark");
  });
});
