// 子树 CPU 展示门槛的回归（KNOWN-GAPS Gap 1/B）。
// 这两个阈值是产品决策，不是实现细节：徽标的意义是「本行看着闲、负载全在子进程里」
// 这一条反直觉的情形 —— 自身就在满核的健康构建（vite build / tsc）绝不能挂徽标，
// 否则日常开发中它会常驻在半数行上，从示警退化成噪音。
import { describe, it, expect } from "vite-plus/test";
import { hasBusySubtree, subtreeCpuExceedsSelf, whitelistKey, type ProcessEntry } from "./model";

function entry(cpu: number, tree: number): ProcessEntry {
  return {
    pid: 1,
    ppid: 1,
    ports: [],
    command: "Google Chrome",
    full_command: "Google Chrome --headless=new",
    exe_path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    app_label: "Google Chrome · headless",
    app_category: "automation-instance",
    parent_chain: [],
    launcher_label: "launchd",
    user: "x",
    tty: "",
    elapsed_secs: 25_000,
    start_unix: 1000,
    cpu_percent: cpu,
    cpu_percent_tree: tree,
    mem_mb: 100,
    state: "S",
    is_zombie_suspect: true,
    confidence: "confirmed",
    zombie_reasons: ["ppid1_orphan", "automation_instance"],
    is_whitelisted: false,
    whitelist_key: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    duplicate_of: null,
  };
}

describe("subtree CPU surfacing", () => {
  it("Gap 1 主案形态：主进程 0%、子树 99% ⇒ 徽标与详情都出", () => {
    const e = entry(0.4, 99.6);
    expect(subtreeCpuExceedsSelf(e)).toBe(true);
    expect(hasBusySubtree(e)).toBe(true);
  });

  it("自身就在满核的健康构建：子树不高于自身 ⇒ 不挂徽标", () => {
    const e = entry(180, 180);
    expect(subtreeCpuExceedsSelf(e)).toBe(false);
    expect(hasBusySubtree(e)).toBe(false);
  });

  it("子进程只占一点点：超过死区但达不到示警门槛 ⇒ 只进详情，不占行内", () => {
    const e = entry(1, 12);
    expect(subtreeCpuExceedsSelf(e)).toBe(true);
    expect(hasBusySubtree(e)).toBe(false);
  });

  it("1 个百分点的死区吸收采样抖动", () => {
    expect(subtreeCpuExceedsSelf(entry(2, 2.5))).toBe(false);
    expect(subtreeCpuExceedsSelf(entry(2, 3.5))).toBe(true);
  });

  it("字段缺失（陈旧后端 / 精简夹具）时优雅退化，不渲染 NaN", () => {
    const stale = { ...entry(5, 5) } as Partial<ProcessEntry> as ProcessEntry;
    delete (stale as { cpu_percent_tree?: number }).cpu_percent_tree;
    expect(subtreeCpuExceedsSelf(stale)).toBe(false);
    expect(hasBusySubtree(stale)).toBe(false);
  });
});

// 引擎自 core 拆分起随每行输出 whitelist_key；前端的 whitelistKey() 是历史实现，
// 两份并存就必须有东西钉住它们一致 —— 否则「在 Raycast 加的星桌面版认不出来」
// 这类 bug 只会在用户手里出现（推导规则有一条反直觉分支：exe_path 仅在含路径
// 分隔符时可用，PATH 解析出的裸解释器名 `node` 会塌缩掉全机同名监听者）。
describe("whitelistKey 与引擎产出的 whitelist_key 一致", () => {
  it("exe_path 含分隔符时用 exe_path", () => {
    const e = entry(0, 0);
    expect(whitelistKey(e)).toBe(e.whitelist_key);
  });

  it("裸解释器名（无分隔符）时回退到完整命令行", () => {
    const e: ProcessEntry = {
      ...entry(0, 0),
      exe_path: "node",
      full_command: "node /Users/x/app/server.js",
      whitelist_key: "node /Users/x/app/server.js",
    };
    expect(whitelistKey(e)).toBe(e.whitelist_key);
  });
});
