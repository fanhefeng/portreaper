// check-paths-parity.mjs 的自测（node --test scripts/*.test.mjs）。
// 守卫脚本自己也是代码：每条规则用「真实源码 + 定向突变」验证能抓住对应回归。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkPathsParity,
  extractCoreIdentifier,
  extractTauriIdentifier,
} from "./check-paths-parity.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  pathsSrc: readFileSync(join(root, "crates/portreaper-core/src/paths.rs"), "utf8"),
  tauriConfSrc: readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"),
};

test("真实源码当前必须通过校验", () => {
  assert.deepEqual(checkPathsParity(real), []);
});

test("identifier 分叉必须被拦截", () => {
  const tauriConfSrc = real.tauriConfSrc.replace(
    /"identifier"\s*:\s*"[^"]*"/,
    '"identifier": "com.evil.other"',
  );
  const errors = checkPathsParity({ ...real, tauriConfSrc });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /identifier 不一致/);
});

test("core 常量被改名时必须响亮失败，而不是静默放行", () => {
  const pathsSrc = real.pathsSrc.replace("pub const APP_IDENTIFIER", "pub const APP_ID");
  assert.throws(() => checkPathsParity({ ...real, pathsSrc }), /找不到/);
});

test("tauri.conf.json 缺 identifier 时必须响亮失败", () => {
  assert.throws(() => extractTauriIdentifier("{}"), /缺少顶层 identifier/);
});

test("两侧都能从真实源码解析出同一个值", () => {
  const core = extractCoreIdentifier(real.pathsSrc);
  assert.equal(core, extractTauriIdentifier(real.tauriConfSrc));
  assert.match(core, /^[a-z0-9.-]+$/i);
});
