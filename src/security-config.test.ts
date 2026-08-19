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
import { describe, it, expect } from "vite-plus/test";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

// happy-dom 环境下 import.meta.url 是 http: 协议，不能喂给 fs ——
// vitest 的工作目录就是项目根，直接从 cwd 解析。
const conf = JSON.parse(readFileSync(join(process.cwd(), "src-tauri/tauri.conf.json"), "utf8"));

// **必须枚举整个目录，不能只读 default.json**（评审发现）：tauri.conf.json 没有设
// `app.security.capabilities`，该字段缺省时 Tauri 2 会启用 `capabilities/` 目录下的
// **全部**文件。只断言 default.json 的话，新增一个 extra.json 放行 shell:allow-execute
// / fs:default 能通过本测试、typecheck、全部守卫脚本与 clippy —— 攻击面已经打开，
// 而这几条断言正是为「新增权限必须先改这里、强制过一次评审」而存在的。
const CAPS_DIR = join(process.cwd(), "src-tauri/capabilities");
const capFiles = readdirSync(CAPS_DIR)
  .filter((f) => f.endsWith(".json"))
  .sort();
/** 真正生效的权限面 = 目录下所有 capability 文件的 permissions 并集。 */
const allPermissions: unknown[] = capFiles.flatMap(
  (f) => JSON.parse(readFileSync(join(CAPS_DIR, f), "utf8")).permissions ?? [],
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
    // 'unsafe-eval' 同理：构建产物里没有 eval / new Function，放行它只是白送
    // 一条注入后的代码执行路径。整串查即可 —— 它出现在任何指令里都不可接受。
    expect(s).not.toContain("'unsafe-eval'");
    // frame-ancestors 没有 default-src 回退（object-src 有，故不必单列）——
    // 不写死它，一次意外的 iframe 嵌入就没有任何东西拦得住。
    expect(s).toContain("frame-ancestors 'none'");
  });

  it("三个未设防的侧面：不得关掉 CSP 注入、不得开 assetProtocol、不得另开 devCsp", () => {
    const security: Record<string, unknown> = conf.app?.security ?? {};
    // 关掉它，Tauri 为自身资源注入的 nonce / hash 源全部失效 —— 上面那条
    // 「CSP 非空」照样通过，而实际保护已经归零。
    expect(security.dangerousDisableAssetCspModification).toBeUndefined();
    // asset: 协议把本地文件暴露给 webview。本应用一个字节的本地文件都不需要读，
    // 而它承载的是用户完整命令行 —— 开它是纯负债。
    const asset = security.assetProtocol as { enable?: boolean } | undefined;
    expect(asset?.enable ?? false).toBe(false);
    // devCsp 缺省时 dev 沿用 csp。单独给 dev 放宽 = 平时跑的根本不是发布态的
    // 安全配置，问题只会在发版那天出现。
    expect(security.devCsp).toBeUndefined();
  });

  it("capabilities 目录只有 default.json —— 多一个文件就是多一份被静默启用的权限", () => {
    // 这条与下面的并集断言是两道独立的闸：即便有人连同本断言一起改了文件集，
    // 并集断言仍会因为新权限而失败。
    expect(capFiles).toEqual(["default.json"]);
  });

  it("权限面是精确的全量白名单：core:default + log:default + scoped opener，不多一项", () => {
    const perms: unknown[] = allPermissions;
    // 字符串型权限精确等于期望集合 —— 新增任何权限必须显式修改本断言。
    // log:default：前端 logger.ts 把未捕获异常转发到后端日志文件所需（仅写日志，
    // 无读取/无文件系统访问，攻击面极小）。
    const stringPerms = perms.filter((p): p is string => typeof p === "string");
    expect(stringPerms).toEqual(["core:default", "log:default"]);
    // 对象型权限只有一个：scoped 的 opener:allow-open-url（下一测试校验 scope）。
    // 直接数对象项，不写 perms.length === 3 —— 那个总数把「字符串型 2 条」的信息
    // 重复了一遍，将来字符串集合一变，这行就要跟着改，且失败信息说不清是哪一侧多了。
    const objectPerms = perms.filter((p) => typeof p === "object" && p !== null);
    expect(objectPerms.length).toBe(1);
  });

  it("opener 权限已收窄：无 opener:default，open-url 仅限 localhost", () => {
    const perms: unknown[] = allPermissions;
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
