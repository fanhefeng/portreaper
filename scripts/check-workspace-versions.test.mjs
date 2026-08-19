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

test("解析到的钉版数量与文件自述的「共 12 处」一致", () => {
  // 数量本身也是断言：将来加/删平台包时，这条会逼人回来同步那句注释。
  assert.equal(collectPinnedVersions(real).length, 12);
});

test("漏改一条 exclude 必须被拦截 —— 这正是文件里写着「会静默失效」的那一条", () => {
  const mutated = real.replace(
    '- "@voidzero-dev/vite-plus-linux-x64-gnu@0.2.6"',
    '- "@voidzero-dev/vite-plus-linux-x64-gnu@0.2.5"',
  );
  assert.notEqual(mutated, real, "突变未生效：锚点条目已变，用例形同虚设");
  const errors = checkWorkspaceVersions(mutated);
  assert.equal(errors.length, 1);
  assert.match(errors[0], /不一致/);
  assert.match(errors[0], /linux-x64-gnu/);
});

test("只改了 catalog 而 exclude 全没跟上，同样被拦截", () => {
  const mutated = real.replace("vite-plus: 0.2.6", "vite-plus: 0.3.0");
  assert.notEqual(mutated, real, "突变未生效");
  assert.match(checkWorkspaceVersions(mutated)[0], /不一致/);
});

test("注释里的版本号不参与比对 —— 那句警告自己就带着一个", () => {
  // 注释写 0.2.6、实际全是 0.3.0：只要真实钉版自洽就该通过
  const mutated = real
    .replaceAll("0.2.6", "0.3.0")
    .replace("共 12 处", "共 12 处（注释仍写 0.2.6）");
  assert.deepEqual(checkWorkspaceVersions(mutated), []);
});

test("文件形态变到解析不出任何钉版时响亮失败，而不是「没找到 = 没问题」", () => {
  assert.match(checkWorkspaceVersions("packages:\n  - '.'\n")[0], /一条钉版都没解析到/);
});
