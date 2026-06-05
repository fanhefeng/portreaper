// 安全配置回归守卫（评审发现的两项纵深防御，防止被无意回退）：
// - CSP：生产构建必须有 CSP（dev 走 devCsp，不受影响）；connect-src 必须
//   放行 Tauri IPC 的两种承载（macOS/Linux 的 ipc: 与 Windows 的
//   http://ipc.localhost），否则发布版直接白屏。
// - opener scope：唯一的 openUrl 调用点是「在浏览器打开 http://localhost:<port>」，
//   权限必须收窄到该模式；裸的 opener:default / 无 scope 的 allow-open-url
//   会放行任意 http/https URL（webview 一旦被注入即可钓鱼跳转）。
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
