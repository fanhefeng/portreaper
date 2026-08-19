// check-tauri-parity.mjs 的自测（node --test scripts/*.test.mjs）。
// 守卫脚本自己也是代码：每条规则用「真实源码 + 定向突变」验证能抓住对应回归。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkTauriParity,
  derivePairs,
  extractCrateVersion,
  extractNpmVersion,
  majorMinor,
} from "./check-tauri-parity.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkgJson = readFileSync(join(root, "package.json"), "utf8");
const real = {
  cargoLock: readFileSync(join(root, "Cargo.lock"), "utf8"),
  pnpmLock: readFileSync(join(root, "pnpm-lock.yaml"), "utf8"),
  pairs: derivePairs(pkgJson),
};

test("新增 npm 插件自动纳管 —— 手工表时代漏加一对不会被任何东西发现", () => {
  // 评审指出的漏洞：加一对 @tauri-apps/plugin-dialog + tauri-plugin-dialog、
  // 版本随便错开，旧的手工表照样返回 0 个错误。推导表让新插件自动进入比对面。
  const pkg = JSON.parse(pkgJson);
  pkg.dependencies["@tauri-apps/plugin-dialog"] = "^2.0.0";
  const pairs = derivePairs(JSON.stringify(pkg));
  assert.ok(
    pairs.some((p) => p.crate === "tauri-plugin-dialog" && p.npm === "@tauri-apps/plugin-dialog"),
    `新插件未被推导进配对表：${JSON.stringify(pairs)}`,
  );
});

test("认不出的 @tauri-apps 包响亮失败 —— 不静默跳过一个可能有 Rust 侧的包", () => {
  const pkg = JSON.parse(pkgJson);
  pkg.dependencies["@tauri-apps/something-new"] = "^1.0.0";
  assert.throws(() => derivePairs(JSON.stringify(pkg)), /认不出的 @tauri-apps 包/);
});

test("纯 Rust 侧的插件不进表 —— 它们没有可漂移的两侧", () => {
  const pairs = derivePairs(pkgJson);
  for (const crate of ["tauri-plugin-updater", "tauri-plugin-window-state"]) {
    assert.ok(!pairs.some((p) => p.crate === crate), `${crate} 不该进配对表`);
  }
});

test("真实源码当前必须通过校验", () => {
  assert.deepEqual(checkTauriParity(real), []);
});

// 本守卫存在的**原因**：v0.9.0 的三条 release 构建腿同时炸在
// "Found version mismatched Tauri packages" 上 —— dependabot 把 Rust 侧
// tauri-plugin-log 抬到 2.9.0，npm 侧 @tauri-apps/plugin-log 还停在 2.8.0，
// 而 CI 与 pre-push 都不跑 tauri build，一路全绿合进了 main。
test("回归：只升 Rust 侧的 tauri 插件（v0.9.0 的事故形态）必须被拦截", () => {
  const cargoLock = real.cargoLock.replace(
    /(\[\[package\]\]\nname = "tauri-plugin-log"\nversion = ")[^"]+/,
    "$12.99.0",
  );
  assert.notEqual(cargoLock, real.cargoLock, "突变未生效：Cargo.lock 形态已变，用例形同虚设");
  const errors = checkTauriParity({ ...real, cargoLock });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /tauri-plugin-log/);
  assert.match(errors[0], /2\.99/);
  assert.match(errors[0], /tauri build/);
});

test("反向漂移（只升 npm 侧）同样必须被拦截", () => {
  const pnpmLock = real.pnpmLock.replace(
    /('@tauri-apps\/plugin-opener':\n\s*specifier:[^\n]*\n\s*version: )[^\s(]+/,
    "$19.9.9",
  );
  assert.notEqual(pnpmLock, real.pnpmLock, "突变未生效：pnpm-lock 形态已变，用例形同虚设");
  const errors = checkTauriParity({ ...real, pnpmLock });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /@tauri-apps\/plugin-opener/);
});

test("patch 漂移是允许的 —— tauri 自己的检查也只看到 minor", () => {
  const cargoLock = real.cargoLock.replace(
    /(\[\[package\]\]\nname = "tauri"\nversion = "2\.11\.)\d+/,
    "$199",
  );
  assert.notEqual(cargoLock, real.cargoLock, "突变未生效：tauri 已不是 2.11.x，用例需重写");
  assert.deepEqual(checkTauriParity({ ...real, cargoLock }), []);
});

// 依赖被删掉时守卫必须喊出来：静默放行比没有守卫更危险 —— 配对表会悄悄失效，
// 而失效的那一刻正是有人在动依赖、最该被检查的时候（同 check-toolchain-parity）。
test("crate 不存在时响亮失败，绝不静默放行", () => {
  assert.throws(
    () => extractCrateVersion(real.cargoLock, "tauri-plugin-does-not-exist"),
    /找不到 crate/,
  );
});

test("npm 包不存在时响亮失败，绝不静默放行", () => {
  assert.throws(() => extractNpmVersion(real.pnpmLock, "@tauri-apps/nope"), /找不到 npm 包/);
});

// 短名 crate 会在别的包的 dependencies 列表里再次出现，按 [[package]] 分块
// 解析正是为了不读到邻居的版本。
test("按 [[package]] 分块取版本，不被 dependencies 列表里的同名项带偏", () => {
  const lock = [
    '[[package]]\nname = "some-other"\nversion = "9.9.9"\ndependencies = [\n "tauri",\n]\n',
    '[[package]]\nname = "tauri"\nversion = "2.11.5"\n',
  ].join("\n");
  assert.equal(extractCrateVersion(lock, "tauri"), "2.11.5");
});

test("majorMinor 解析不出版本时抛错，不返回 undefined", () => {
  assert.equal(majorMinor("2.11.5", "x"), "2.11");
  assert.throws(() => majorMinor("nightly", "x"), /解析不出/);
});

test("配对表不含 @tauri-apps/cli —— 它是构建工具，没有对应 crate", () => {
  assert.ok(!derivePairs(pkgJson).some((p) => p.npm === "@tauri-apps/cli"));
});
