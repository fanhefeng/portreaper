// Self-test for bump-version.mjs — the riskiest guard script (regex surgery on
// TOML/lock files). Run via `node --test scripts/*.test.mjs`.
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  SEMVER_RE,
  setCargoTomlVersion,
  findCargoTomlVersion,
  setCargoLockVersion,
  setAllCargoLockVersions,
  findCargoLockVersion,
  LOCK_CRATE_NAMES,
  setJsonVersion,
} from "./bump-version.mjs";

test("setJsonVersion 逐字节保留原格式（回归：JSON.stringify 重写曾抹掉 oxfmt 折叠风格，v0.7.0 发版 CI 变红）", () => {
  const raw = '{\n  "version": "0.6.0",\n  "bundle": { "targets": ["app", "dmg", "nsis"] }\n}\n';
  const out = setJsonVersion(raw, "0.7.0");
  // 除 version 值外一个字节不变：折叠的数组必须保持折叠
  assert.equal(
    out,
    '{\n  "version": "0.7.0",\n  "bundle": { "targets": ["app", "dmg", "nsis"] }\n}\n',
  );
});

test("setJsonVersion 无 version 键或替换未生效时返回 null（响亮失败，不静默写坏）", () => {
  assert.equal(setJsonVersion('{ "name": "x" }', "1.0.0"), null);
  // 首个匹配是嵌套 version（顶层没有）→ 顶层校验不过 → null
  assert.equal(setJsonVersion('{ "dep": { "version": "1.0.0" } }', "2.0.0"), null);
});

test("SEMVER_RE accepts valid versions (incl. pre-release/build)", () => {
  for (const v of [
    "1.2.3",
    "0.5.1",
    "10.20.30",
    "0.0.0",
    "1.2.3-beta.1",
    "1.0.0+build.5",
    "1.2.3-rc.1+sha.abc",
  ]) {
    assert.ok(SEMVER_RE.test(v), `should accept ${v}`);
  }
});

test("SEMVER_RE rejects leading zeros and malformed versions (D1)", () => {
  for (const v of [
    "01.2.3",
    "1.02.3",
    "1.2.03",
    "1.2",
    "1.2.3.4",
    "v1.2.3",
    "a.b.c",
    "",
    "1.2.x",
  ]) {
    assert.ok(!SEMVER_RE.test(v), `should reject ${v}`);
  }
});

test("setCargoTomlVersion rewrites [package] version only, not dependencies", () => {
  const toml = [
    "[package]",
    'name = "portreaper"',
    'version = "0.5.1"',
    'edition = "2021"',
    "",
    "[dependencies]",
    'serde = "1.0.200"',
  ].join("\n");
  const out = setCargoTomlVersion(toml, "0.6.0");
  assert.equal(findCargoTomlVersion(out), "0.6.0");
  assert.ok(out.includes('serde = "1.0.200"'), "dependency version untouched");
  assert.ok(!out.includes('version = "0.5.1"'), "old package version gone");
});

test("setCargoTomlVersion ignores a version in an earlier dependency table", () => {
  const toml = [
    "[dependencies]",
    'foo = { version = "1.2.3" }',
    "",
    "[package]",
    'version = "0.5.1"',
  ].join("\n");
  const out = setCargoTomlVersion(toml, "0.6.0");
  assert.ok(out.includes('foo = { version = "1.2.3" }'), "dependency untouched");
  assert.equal(findCargoTomlVersion(out), "0.6.0");
});

test("setCargoLockVersion targets the portreaper block, not portreaper_lib/serde", () => {
  const lock = [
    "[[package]]",
    'name = "portreaper_lib"',
    'version = "0.5.1"',
    "",
    "[[package]]",
    'name = "portreaper"',
    'version = "0.5.1"',
    "dependencies = [",
    ' "portreaper_lib",',
    "]",
    "",
    "[[package]]",
    'name = "serde"',
    'version = "1.0.200"',
  ].join("\n");
  const out = setCargoLockVersion(lock, "0.6.0");
  assert.equal(findCargoLockVersion(out), "0.6.0");
  assert.ok(
    out.includes('name = "portreaper_lib"\nversion = "0.5.1"'),
    "portreaper_lib block untouched (exact-name match)",
  );
  assert.ok(out.includes('name = "serde"\nversion = "1.0.200"'), "serde block untouched");
});

// 清单形态被破坏时必须响亮失败 —— 这几条分支曾经调 process.exit(1)，
// 断言碰不到它们（一碰就把测试进程带走），等于「守住 bump 正确性」的最后一环无人验证。
test("Cargo.toml 缺 [package] 段必须抛错，而不是返回原文", () => {
  assert.throws(() => setCargoTomlVersion('[dependencies]\nfoo = "1"\n', "0.6.0"), /\[package\]/);
});

test("Cargo.toml 的 [package] 段缺 version 行必须抛错", () => {
  assert.throws(() => setCargoTomlVersion('[package]\nname = "portreaper"\n', "0.6.0"), /version/);
});

test("Cargo.lock 找不到目标包块必须抛错", () => {
  const lock = ["[[package]]", 'name = "serde"', 'version = "1.0.200"'].join("\n");
  assert.throws(() => setCargoLockVersion(lock, "0.6.0"), /portreaper/);
});

test("Cargo.lock 目标包块缺 version 行必须抛错", () => {
  const lock = ["[[package]]", 'name = "portreaper"', "dependencies = []"].join("\n");
  assert.throws(() => setCargoLockVersion(lock, "0.6.0"), /version line/);
});

// ---- 多发布产物：portreaper 与 portreaper-cli 必须一起同步 ----
// 判据是「用户能不能看见这个版本号」：安装包与 release 里的 CLI 都看得见，
// 内部库 portreaper-core 看不见。曾经 CLI 自成一套（0.1.0），用户 `--version`
// 读到的号对应不到任何一个 release。

const MULTI_LOCK = [
  "[[package]]",
  'name = "portreaper"',
  'version = "0.5.1"',
  "",
  "[[package]]",
  'name = "portreaper-cli"',
  'version = "0.1.0"',
  "",
  "[[package]]",
  'name = "portreaper-core"',
  'version = "0.1.0"',
].join("\n");

test("setAllCargoLockVersions 同步全部发布产物，且不碰内部库", () => {
  const out = setAllCargoLockVersions(MULTI_LOCK, "0.9.0");
  assert.equal(findCargoLockVersion(out, "portreaper"), "0.9.0");
  assert.equal(findCargoLockVersion(out, "portreaper-cli"), "0.9.0");
  assert.equal(
    findCargoLockVersion(out, "portreaper-core"),
    "0.1.0",
    "portreaper-core 不发布，版本不应被同步",
  );
});

// 行尾锚定的回归：没有 `$`，正则 `name = "portreaper"` 会连 "portreaper-cli"
// 的块一起命中，于是两个包都被当成第一个处理 —— 改对一个、漏掉另一个，且不报错。
test("包名匹配必须精确，portreaper 不得命中 portreaper-cli 的块", () => {
  const out = setCargoLockVersion(MULTI_LOCK, "0.9.0", "portreaper");
  assert.equal(findCargoLockVersion(out, "portreaper"), "0.9.0");
  assert.equal(
    findCargoLockVersion(out, "portreaper-cli"),
    "0.1.0",
    "只改 portreaper 时，portreaper-cli 必须原封不动",
  );
});

// 幂等回归（实测踩到）：同版本重跑时 replace 是 no-op，旧守卫把「结果与原文相同」
// 一律当成「version 行缺失」—— 中断后的重跑会在 Cargo.lock 一步炸出误导性错误，
// 而 setCargoTomlVersion 对同样的 no-op 却静默通过（两函数判据必须一致）。
test("Cargo.lock 已是目标版本时重跑必须返回原文，不得误报缺 version 行", () => {
  const lock = ["[[package]]", 'name = "portreaper"', 'version = "0.9.0"'].join("\n");
  assert.equal(setCargoLockVersion(lock, "0.9.0"), lock);
  // 全量同步的重跑同样幂等
  const synced = setAllCargoLockVersions(MULTI_LOCK, "0.9.0");
  assert.equal(setAllCargoLockVersions(synced, "0.9.0"), synced);
});

test("LOCK_CRATE_NAMES 就是发布产物的清单", () => {
  assert.deepEqual(LOCK_CRATE_NAMES, ["portreaper", "portreaper-cli"]);
});
