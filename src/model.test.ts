// 子树 CPU 展示门槛的回归（KNOWN-GAPS Gap 1/B）。
// 这两个阈值是产品决策，不是实现细节：徽标的意义是「本行看着闲、负载全在子进程里」
// 这一条反直觉的情形 —— 自身就在满核的健康构建（vite build / tsc）绝不能挂徽标，
// 否则日常开发中它会常驻在半数行上，从示警退化成噪音。
import { describe, it, expect } from "vite-plus/test";
import {
  formatStartTime,
  hasBusySubtree,
  isSameProcess,
  localizeKillError,
  safeVerdict,
  subtreeCpuExceedsSelf,
  type ProcessEntry,
  type Translator,
} from "./model";
import { makeEntry } from "./test-fixtures";

/** Gap 1 主案形态的语义化夹具：headless Chrome 自动化实例，CPU 两值参数化 */
function entry(cpu: number, tree: number): ProcessEntry {
  return makeEntry({
    ports: [],
    command: "Google Chrome",
    full_command: "Google Chrome --headless=new",
    exe_path: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    app_label: "Google Chrome · headless",
    app_category: "automation-instance",
    elapsed_secs: 25_000,
    cpu_percent: cpu,
    cpu_percent_tree: tree,
    mem_mb: 100,
    zombie_reasons: ["ppid1_orphan", "automation_instance"],
    whitelist_key: "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  });
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

// kill 错误的 wire 契约（issue #35）：引擎按 `#[serde(tag = "code")]` 过 IPC，
// 前端按 code 分派。此前是 `includes("ERR_…")` 子串匹配，漏改只会静默退化成
// 「透传英文原文」—— 这组测试钉的就是那条退化路径不许再出现。
describe("localizeKillError", () => {
  // 只回显 key，断言「分派到了哪一支」而非具体译文（译文改动不该弄红这组测试）
  const t = ((key: string) => key) as unknown as Translator;

  it("四个语义 code 各自分派到对应的本地化键", () => {
    const cases = [
      ["pid_reused", "error.pidReused"],
      ["process_gone", "error.processGone"],
      ["access_denied", "error.accessDenied"],
      ["identity_unknown", "error.identityUnknown"],
    ] as const;
    for (const [code, key] of cases) {
      expect(localizeKillError({ code }, t)).toBe(key);
    }
  });

  it("os 变体无语义，原样展示系统原文（不得被本地化吞掉）", () => {
    expect(localizeKillError({ code: "os", message: "Operation not permitted" }, t)).toBe(
      "Operation not permitted",
    );
  });

  it("超时 sentinel 不是 KillError，仍走 localizeActionError", () => {
    expect(localizeKillError(new Error("ERR_ACTION_TIMEOUT"), t)).toBe("error.actionTimeout");
  });

  it("认不出的形态一律透传，绝不吞成空横幅", () => {
    // 未知 code（后端比前端新）：退回原文，用户至少看得见发生了什么
    expect(localizeKillError({ code: "brand_new_variant" }, t)).toContain("brand_new_variant");
    // message 为空的 os 变体：不能渲染出一条空错误
    expect(localizeKillError({ code: "os", message: "" }, t)).not.toBe("");
  });

  it("回归：结构化错误被 String() 压平后不得再命中语义分支", () => {
    // App.tsx 曾对 err 套一层 String()。压平后是 "[object Object]"，
    // 这里断言它确实分派不出语义分支 —— 一旦有人重新加回 String()，
    // 上面第一条测试会立刻变红，而不是在真机上静默退化成英文原文。
    expect(localizeKillError(String({ code: "pid_reused" }), t)).not.toBe("error.pidReused");
  });
});

// 注：此处曾有一组「whitelistKey() 与引擎产出的 whitelist_key 一致」的测试。
// 前端那份手推实现已删除（App.tsx 改为直读 ProcessEntry.whitelist_key，与
// Raycast 侧同一约定），只剩一份真相源后一致性断言便失去了对象 —— 键推导规则
// 本身由 Rust 侧 scanner::mod::helper_tests 覆盖。

// 「两次扫描里的这一行还是同一个进程吗」——终止后存活确认的地基。
// 用严格相等会随机误判（start_unix 由 now-etime 推导，秒级粒度导致 ±1s 抖动），
// 而误判的方向恰好是「进程已消失」，也就是把杀不掉的进程报成杀掉了。
describe("isSameProcess", () => {
  it("±1s 抖动内仍是同一个进程", () => {
    const e = makeEntry({ pid: 4242, start_unix: 1000 });
    expect(isSameProcess(e, 4242, 1000)).toBe(true);
    expect(isSameProcess(e, 4242, 999)).toBe(true);
    expect(isSameProcess(e, 4242, 1001)).toBe(true);
    expect(isSameProcess(e, 4242, 1005)).toBe(true); // 容差边界（内侧）
  });

  it("超出容差即不算同一个进程 —— 上界也要钉住，否则放宽容差不会翻红", () => {
    const e = makeEntry({ pid: 4242, start_unix: 1000 });
    expect(isSameProcess(e, 4242, 1006)).toBe(false);
    expect(isSameProcess(e, 4242, 994)).toBe(false);
  });

  it("PID 被复用（创建时间晚得多）不算同一个进程", () => {
    const recycled = makeEntry({ pid: 4242, start_unix: 9000 });
    expect(isSameProcess(recycled, 4242, 1000)).toBe(false);
  });

  it("PID 不同一律不算", () => {
    expect(isSameProcess(makeEntry({ pid: 1 }), 2, 1000)).toBe(false);
  });

  it("缺令牌时退化为只认 PID（没有可比的东西，宁可认成同一个也不谎报已消失）", () => {
    expect(isSameProcess(makeEntry({ pid: 7, start_unix: null }), 7, 1000)).toBe(true);
    expect(isSameProcess(makeEntry({ pid: 7, start_unix: 1000 }), 7, null)).toBe(true);
  });
});

// 「能否清理」处置建议：五档全部锚定清扫策略（starred/healthy 不在目标集、
// duplicate 与 possible 永不入清扫、confirmed/likely 恰是 SWEEPABLE）。
// 分支**顺序**也是语义：星标压过一切 —— 引擎照常发出 confidence，
// 但用户亲手豁免过的行绝不能被建议「可以清」。
describe("safeVerdict", () => {
  it("星标压过引擎判定（哪怕引擎判 confirmed）", () => {
    const e = makeEntry({
      is_whitelisted: true,
      is_zombie_suspect: false,
      confidence: "confirmed",
    });
    expect(safeVerdict(e)).toBe("starred");
  });

  it("健康行不建议动手", () => {
    expect(safeVerdict(makeEntry({ is_zombie_suspect: false, confidence: "none" }))).toBe(
      "healthy",
    );
  });

  it("重复实例交给用户判断 —— 机器不知道在用哪个", () => {
    const e = makeEntry({ is_zombie_suspect: true, confidence: "possible", duplicate_of: 4268 });
    expect(safeVerdict(e)).toBe("duplicate");
  });

  it("possible 证据弱：清扫永不覆盖 ⇒ 谨慎档", () => {
    expect(safeVerdict(makeEntry({ is_zombie_suspect: true, confidence: "possible" }))).toBe(
      "weak",
    );
  });

  it("confirmed / likely 与一键清扫目标集同口径 ⇒ 可以清", () => {
    expect(safeVerdict(makeEntry({ is_zombie_suspect: true, confidence: "confirmed" }))).toBe(
      "yes",
    );
    expect(safeVerdict(makeEntry({ is_zombie_suspect: true, confidence: "likely" }))).toBe("yes");
  });
});

// 绝对启动时间：断言只钉「按语言选了对应 locale」这一层，不钉具体时刻
// （toLocaleString 走本机时区，CI 各机不同；取月中时间戳避免时区跨月）。
describe("formatStartTime", () => {
  const aug15 = Math.floor(Date.UTC(2026, 7, 15, 12, 0, 0) / 1000);

  it("zh 走中文日期形态（含「月」）", () => {
    expect(formatStartTime(aug15, "zh")).toContain("月");
  });

  it("en 走英文月份缩写", () => {
    expect(formatStartTime(aug15, "en")).toMatch(/Aug/);
  });
});
