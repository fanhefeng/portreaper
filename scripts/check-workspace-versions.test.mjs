// check-workspace-versions.mjs 的自测：每条规则用「真实源码 + 定向突变」验证
// 能抓住对应的回归 —— 守卫静默放行比没有守卫更危险。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkWorkspaceVersions, collectPinnedVersions } from "./check-workspace-versions.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = readFileSync(join(root, "pnpm-workspace.yaml"), "utf8");

test("真实源码当前必须通过校验", () => {
  assert.deepEqual(checkWorkspaceVersions(real), []);
});

test("只升级真实 catalog 的一个版本必须被拦截，不依赖当前版本号", () => {
  const pin = collectPinnedVersions(real).find((p) => p.line.startsWith("vite-plus:"));
  assert.ok(pin, "真实 catalog 缺少 vite-plus 钉版");
  const next = `${Number(pin.version.split(".")[0]) + 1}.0.0`;
  const mutated = real.replace(pin.line, pin.line.replace(pin.version, next));
  assert.notEqual(mutated, real, "突变未生效");
  assert.match(checkWorkspaceVersions(mutated)[0], /不一致/);
});

test("真实 catalog 任一必需钉版被删除都必须失败", () => {
  for (const name of ["vite", "vite-plus"]) {
    const pattern = new RegExp(`^  ${name}:.*\\n`, "m");
    const mutated = real.replace(pattern, "");
    assert.notEqual(mutated, real, `${name} 的删除突变未生效`);
    assert.match(checkWorkspaceVersions(mutated).join("\n"), /catalog/);
  }
});

// 可选的发布等待例外用独立样例验证；不要求真实 workspace 保留旧版本的例外。
const catalog = `catalog:
  vite: npm:@voidzero-dev/vite-plus-core@1.2.3
  vite-plus: 1.2.3
`;
const withExcludes = `${catalog}minimumReleaseAgeExclude:
  - "@voidzero-dev/vite-plus-core@1.2.3"
  - "@voidzero-dev/vite-plus-linux-x64-gnu@1.2.3"
  - vite-plus@1.2.3
`;

test("没有发布等待例外时，两个 catalog 钉版一致即通过", () => {
  assert.deepEqual(checkWorkspaceVersions(catalog), []);
  assert.equal(collectPinnedVersions(catalog).length, 2);
});

test("可选发布等待例外与 catalog 一致时通过", () => {
  assert.deepEqual(checkWorkspaceVersions(withExcludes), []);
  assert.equal(collectPinnedVersions(withExcludes).length, 5);
});

test("漏改一条可选 exclude 必须被拦截并指出平台包", () => {
  const mutated = withExcludes.replace("linux-x64-gnu@1.2.3", "linux-x64-gnu@1.2.2");
  assert.notEqual(mutated, withExcludes, "突变未生效");
  const errors = checkWorkspaceVersions(mutated);
  assert.equal(errors.length, 1);
  assert.match(errors[0], /不一致/);
  assert.match(errors[0], /linux-x64-gnu/);
});

test("catalog 一起升级而 exclude 没跟上，同样被拦截", () => {
  const mutated = withExcludes.replace(catalog, catalog.replaceAll("1.2.3", "2.0.0"));
  assert.notEqual(mutated, withExcludes, "突变未生效");
  assert.match(checkWorkspaceVersions(mutated)[0], /不一致/);
});

test("注释里的版本号不参与比对，支持行尾注释与带引号的值", () => {
  const src = `# 旧版本 0.2.6
catalog:
  vite: "npm:@voidzero-dev/vite-plus-core@1.2.3" # 旧版本 0.2.6
  vite-plus: '1.2.3'
`;
  assert.deepEqual(checkWorkspaceVersions(src), []);
});

test("其他依赖和其他段落不参与 Vite+ 版本比对", () => {
  const src = `${withExcludes}  - unrelated-package@9.8.7
overrides:
  vite: 8.0.0
`;
  assert.deepEqual(checkWorkspaceVersions(src), []);
  assert.equal(collectPinnedVersions(src).length, 5);
});

test("版本范围、错误 core 别名和未钉版的 Vite+ 例外必须失败", () => {
  for (const src of [
    catalog.replace("vite-plus: 1.2.3", "vite-plus: ^1.2.3"),
    catalog.replace("npm:@voidzero-dev/vite-plus-core@", "npm:vite@"),
    `${catalog}minimumReleaseAgeExclude:\n  - vite-plus\n`,
    withExcludes.replace("linux-x64-gnu@1.2.3", "linux-x64-gnu@*"),
  ]) {
    assert.notDeepEqual(checkWorkspaceVersions(src), [], src);
  }
});

test("重复 catalog 钉版或仅有 exclude 不能伪满足必需配置", () => {
  assert.match(checkWorkspaceVersions(`${catalog}  vite-plus: 1.2.3\n`).join("\n"), /catalog/);
  assert.match(checkWorkspaceVersions(withExcludes.replace(catalog, "")).join("\n"), /catalog/);
});

test("文件形态变到解析不出任何钉版时响亮失败，而不是「没找到 = 没问题」", () => {
  assert.match(checkWorkspaceVersions("packages:\n  - '.'\n")[0], /一条钉版都没解析到/);
});
