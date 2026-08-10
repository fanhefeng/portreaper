// check-model-parity.mjs 的自测（node --test scripts/*.test.mjs）。
// 守卫脚本自己也是代码：每条规则用「真实源码 + 定向突变」验证能抓住对应回归。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkModelParity,
  checkToleranceParity,
  extractRustFields,
  extractTsFields,
} from "./check-model-parity.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  rustSrc: readFileSync(join(root, "crates/portreaper-core/src/scanner/model.rs"), "utf8"),
  desktopSrc: readFileSync(join(root, "src/model.ts"), "utf8"),
  raycastSrc: readFileSync(join(root, "integrations/raycast/src/cli.ts"), "utf8"),
};

test("真实源码当前必须通过校验", () => {
  assert.deepEqual(checkModelParity(real), []);
});

test("引擎新增字段而镜像未跟上时必须被拦截（两份镜像都要报）", () => {
  const rustSrc = real.rustSrc.replace(
    /(pub whitelist_key: String,)/,
    "$1\n    pub brand_new_field: u32,",
  );
  assert.notEqual(rustSrc, real.rustSrc, "突变未生效：锚点字段已变，用例形同虚设");
  const errors = checkModelParity({ ...real, rustSrc });
  assert.equal(errors.length, 2);
  for (const e of errors) {
    assert.match(e, /缺少引擎字段 \[brand_new_field\]/);
  }
});

test("桌面镜像删字段时必须被拦截，且只报桌面一侧", () => {
  const desktopSrc = real.desktopSrc.replace(/^\s*ppid: number;\n/m, "");
  assert.notEqual(desktopSrc, real.desktopSrc, "突变未生效");
  const errors = checkModelParity({ ...real, desktopSrc });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /src\/model\.ts/);
  assert.match(errors[0], /\[ppid\]/);
});

test("Raycast 镜像字段改名时必须同时报缺失与多出", () => {
  const raycastSrc = real.raycastSrc.replace(/^(\s*)pid: number;/m, "$1pid_renamed: number;");
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效");
  const errors = checkModelParity({ ...real, raycastSrc });
  // ProcessEntry 与 ParentRef 都有 pid 字段，改哪个都必须被抓；
  // replace 只改第一处（文件里先出现的那个类型）
  assert.equal(errors.length, 1);
  assert.match(errors[0], /缺少引擎字段 \[pid\]/);
  assert.match(errors[0], /多出引擎没有的字段 \[pid_renamed\]/);
});

test("ParentRef 的镜像同样纳管", () => {
  const raycastSrc = real.raycastSrc.replace(
    /export type ParentRef = \{\n(\s*)pid: number;/,
    "export type ParentRef = {\n$1pid: number;\n$1ghost: string;",
  );
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效");
  const errors = checkModelParity({ ...real, raycastSrc });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /ParentRef/);
  assert.match(errors[0], /\[ghost\]/);
});

test("Rust 注释掉的旧字段不得被当作真值", () => {
  const rustSrc = real.rustSrc.replace(
    /(pub struct ProcessEntry \{)/,
    "$1\n    // pub stale_old_field: u32,",
  );
  assert.notEqual(rustSrc, real.rustSrc, "突变未生效");
  assert.deepEqual(checkModelParity({ ...real, rustSrc }), []);
});

test("serde rename 以 wire 名计", () => {
  const rustSrc = real.rustSrc.replace(
    /^(\s*)pub pid: u32,\n(\s*)pub ppid: u32,/m,
    '$1#[serde(rename = "pid")]\n$1pub pid_internal: u32,\n$2pub ppid: u32,',
  );
  assert.notEqual(rustSrc, real.rustSrc, "突变未生效");
  // wire 名仍是 pid ⇒ 与镜像依旧一致
  assert.deepEqual(checkModelParity({ ...real, rustSrc }), []);
});

test("serde skip 的字段不计入 wire 契约", () => {
  const rustSrc = real.rustSrc.replace(
    /(pub whitelist_key: String,)/,
    "$1\n    #[serde(skip)]\n    pub internal_only: u32,",
  );
  assert.notEqual(rustSrc, real.rustSrc, "突变未生效");
  assert.deepEqual(checkModelParity({ ...real, rustSrc }), []);
});

test("Rust 结构体被改名时必须响亮失败，而不是静默放行", () => {
  const rustSrc = real.rustSrc.replace("pub struct ProcessEntry", "pub struct ScanRow");
  assert.throws(() => checkModelParity({ ...real, rustSrc }), /找不到/);
});

test("TS 类型被改名时必须响亮失败", () => {
  const desktopSrc = real.desktopSrc.replace("export type ProcessEntry", "export type ScanRow");
  assert.throws(() => checkModelParity({ ...real, desktopSrc }), /找不到/);
});

test("TS 块注释里的伪字段不得被当作真值", () => {
  const desktopSrc = real.desktopSrc.replace(
    /(export type ProcessEntry = \{)/,
    "$1\n  /** fake_in_comment: number; */",
  );
  assert.notEqual(desktopSrc, real.desktopSrc, "突变未生效");
  assert.deepEqual(checkModelParity({ ...real, desktopSrc }), []);
});

test("三份真实源码解析出的 ProcessEntry 字段数一致且非空", () => {
  const rust = extractRustFields(real.rustSrc, "ProcessEntry");
  const desktop = extractTsFields(real.desktopSrc, "ProcessEntry", "src/model.ts");
  const raycast = extractTsFields(real.raycastSrc, "ProcessEntry", "cli.ts");
  assert.ok(rust.length > 10, `字段数异常偏少（${rust.length}）——解析器可能失配`);
  assert.deepEqual([...desktop].sort(), [...rust].sort());
  assert.deepEqual([...raycast].sort(), [...rust].sort());
  assert.ok(rust.includes("pid"));
});

// 身份容差三处必须同值 —— 放宽它会让被复用的 PID 被认成同一个进程。
// 此前三处只有互相声明「取值一致」的注释，改一处不会让任何检查变红（评审发现）。
const TOL = {
  platformSrc: "const START_TOLERANCE_SECS: u64 = 5;",
  desktopSrc: "export const START_MATCH_TOLERANCE_SECS = 5;",
  raycastSrc: "export const START_MATCH_TOLERANCE_SECS = 5;",
};

test("三处容差同值时通过", () => {
  assert.deepEqual(checkToleranceParity(TOL), []);
});

test("任一处被改宽即失败", () => {
  const errs = checkToleranceParity({
    ...TOL,
    raycastSrc: "export const START_MATCH_TOLERANCE_SECS = 10;",
  });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /不一致/);
});

test("三处一起改宽同样失败 —— 只校验「相等」会让 CLAUDE.md 的 ±5s 静默变成假话", () => {
  const widened = Object.fromEntries(
    Object.entries(TOL).map(([k, v]) => [k, v.replace("= 5;", "= 10;")]),
  );
  const errs = checkToleranceParity(widened);
  assert.equal(errs.length, 1);
  assert.match(errs[0], /必须是 5 秒/);
});

test("常量被改名/挪走时响亮失败，而不是「没找到 = 没问题」", () => {
  const errs = checkToleranceParity({ ...TOL, desktopSrc: "export const RENAMED = 5;" });
  assert.equal(errs.length, 1);
  assert.match(errs[0], /找不到身份容差常量/);
});
