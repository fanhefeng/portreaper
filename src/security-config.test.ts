// 安全配置回归守卫（评审发现的纵深防御，防止被无意回退）：
// - CSP：生产构建必须有 CSP（Tauri 2 中 devCsp 缺省时 dev 同样套用 csp）；
//   connect-src 必须放行 Tauri IPC 的两种承载（macOS/Linux 的 ipc: 与 Windows 的
//   http://ipc.localhost），否则发布版直接白屏。样式由 Vite 抽成独立 .css 经
//   <link> 引入（style-src 'self' 已覆盖），代码无任何 inline style / CSS-in-JS；
//   脚本经 <script src> 引入 —— 故 style-src 与 script-src 都不得放行 'unsafe-inline'，
//   锁死严格 CSP（评审发现：曾无谓引入 style-src 'unsafe-inline'，无任何消费者）。
// - opener scope：唯一的 openUrl 调用点是「在浏览器打开 http://localhost:<port>」，
//   权限必须收窄到该模式；裸的 opener:default / 无 scope 的 allow-open-url
//   会放行任意 http/https URL（webview 一旦被注入即可钓鱼跳转）。
// - 权限全量白名单：capabilities 只约束 webview 发起的 IPC，而前端只调
//   invoke + openUrl —— 任何新增权限（如曾经的 4 个 core:window:allow-* 死权限，
//   评审发现：无消费者、徒增注入后攻击面）都必须先改这里、强制过一次评审。
import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// happy-dom 环境下 import.meta.url 是 http: 协议，不能喂给 fs ——
// vitest 的工作目录就是项目根，直接从 cwd 解析。
const conf = JSON.parse(
  readFileSync(join(process.cwd(), "src-tauri/tauri.conf.json"), "utf8"),
);
const caps = JSON.parse(
  readFileSync(join(process.cwd(), "src-tauri/capabilities/default.json"), "utf8"),
);

describe("security config guards", () => {
  it("生产 CSP 非空，且 connect-src 覆盖 Tauri IPC 两种承载", () => {
    const csp: unknown = conf.app?.security?.csp;
    expect(typeof csp).toBe("string");
    const s = csp as string;
    expect(s).toContain("default-src 'self'");
    // 缺任一条 IPC 通道都会让发布版 invoke 全挂（dev 不受影响，难以提前发现）
    expect(s).toMatch(/connect-src[^;]*\bipc:/);
    expect(s).toMatch(/connect-src[^;]*http:\/\/ipc\.localhost/);
    // 样式经 <link>、脚本经 <script src> 引入：两者都不得放行 inline ——
    // 否则等于无谓削弱 CSP（webview 一旦被注入即可执行 inline style/script）
    expect(s).not.toMatch(/style-src[^;]*'unsafe-inline'/);
    expect(s).not.toMatch(/script-src[^;]*'unsafe-inline'/);
  });

  it("权限面是精确的全量白名单：core:default + scoped opener，不多一项", () => {
    const perms: unknown[] = caps.permissions;
    // 字符串型权限精确等于期望集合 —— 新增任何权限必须显式修改本断言
    const stringPerms = perms.filter((p): p is string => typeof p === "string");
    expect(stringPerms).toEqual(["core:default"]);
    // 对象型权限只有一个：scoped 的 opener:allow-open-url（下一测试校验 scope）
    expect(perms.length).toBe(2);
  });

  it("opener 权限已收窄：无 opener:default，open-url 仅限 localhost", () => {
    const perms: unknown[] = caps.permissions;
    expect(perms).not.toContain("opener:default");
    expect(perms).not.toContain("opener:allow-open-url"); // 裸形（无 scope）也不允许

    const scoped = perms.find(
      (p): p is { identifier: string; allow: { url: string }[] } =>
        typeof p === "object" &&
        p !== null &&
        (p as { identifier?: string }).identifier === "opener:allow-open-url",
    );
    expect(scoped).toBeTruthy();
    const urls = scoped!.allow.map((a) => a.url);
    // 唯一合法目标：App.tsx handleOpen 的 http://localhost:<port>
    expect(urls).toEqual(["http://localhost:*"]);
  });
});
