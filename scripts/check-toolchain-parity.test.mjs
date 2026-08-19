// check-toolchain-parity.mjs 的自测（node --test scripts/*.test.mjs）。
// 守卫脚本自己也是代码：每条规则用「真实源码 + 定向突变」验证能抓住对应回归。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import {
  checkToolchainParity,
  extractToolchainChannel,
  extractWorkflowToolchains,
} from "./check-toolchain-parity.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  tomlSrc: readFileSync(join(root, "rust-toolchain.toml"), "utf8"),
  ciSrc: readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"),
  releaseSrc: readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
};

test("真实源码当前必须通过校验", () => {
  assert.deepEqual(checkToolchainParity(real), []);
});

test("toml 的 channel 单独升级时必须被拦截", () => {
  const tomlSrc = real.tomlSrc.replace(/channel = "[^"]+"/, 'channel = "1.99.0"');
  assert.notEqual(tomlSrc, real.tomlSrc, "突变未生效：channel 声明形态已变，用例形同虚设");
  const errors = checkToolchainParity({ ...real, tomlSrc });
  // ci.yml 与 release.yml 各至少一条 toolchain: 行，全部应报不一致
  assert.ok(errors.length >= 2);
  assert.match(errors[0], /不一致/);
  assert.match(errors[0], /1\.99\.0/);
});

test("ci.yml 的 toolchain 输入漂移时必须被拦截", () => {
  const ciSrc = real.ciSrc.replace(/toolchain: \S+/, "toolchain: 1.0.0");
  assert.notEqual(ciSrc, real.ciSrc, "突变未生效：toolchain 输入形态已变，用例形同虚设");
  const errors = checkToolchainParity({ ...real, ciSrc });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /ci\.yml/);
});

test("release.yml 的 toolchain 输入漂移时必须被拦截", () => {
  const releaseSrc = real.releaseSrc.replace(/toolchain: \S+/, "toolchain: 1.0.0");
  assert.notEqual(releaseSrc, real.releaseSrc, "突变未生效：toolchain 输入形态已变，用例形同虚设");
  const errors = checkToolchainParity({ ...real, releaseSrc });
  assert.equal(errors.length, 1);
  assert.match(errors[0], /release\.yml/);
});

test("toml 缺 channel 时必须响亮失败，而不是静默放行", () => {
  const tomlSrc = real.tomlSrc.replace(/channel = /, "chan = ");
  assert.notEqual(tomlSrc, real.tomlSrc, "突变未生效");
  assert.throws(() => checkToolchainParity({ ...real, tomlSrc }), /找不到/);
});

test("workflow 里一条 toolchain: 都没有时必须响亮失败", () => {
  const ciSrc = real.ciSrc.replaceAll(/^\s*toolchain: .*$/gm, "");
  assert.notEqual(ciSrc, real.ciSrc, "突变未生效");
  assert.throws(() => checkToolchainParity({ ...real, ciSrc }), /找不到任何/);
});

test("注释掉的旧 channel 不得被当作真值", () => {
  const tomlSrc = real.tomlSrc.replace(/^channel = /m, '# channel = "9.9.9"\nchannel = ');
  assert.notEqual(tomlSrc, real.tomlSrc, "突变未生效");
  assert.deepEqual(checkToolchainParity({ ...real, tomlSrc }), []);
  assert.equal(extractToolchainChannel(tomlSrc), extractToolchainChannel(real.tomlSrc));
});

test("workflow 里注释掉的 toolchain 行不得计入", () => {
  const ciSrc = real.ciSrc.replace(/^(\s*)toolchain: /m, "$1# toolchain: 9.9.9\n$1toolchain: ");
  assert.notEqual(ciSrc, real.ciSrc, "突变未生效");
  assert.deepEqual(checkToolchainParity({ ...real, ciSrc }), []);
});

test("带引号的 toolchain 值也能解析", () => {
  const values = extractWorkflowToolchains('  toolchain: "1.96.0"  # pinned\n', "样例");
  assert.deepEqual(values, ["1.96.0"]);
});

test("不带 toolchain 输入的安装步骤必须被拦下 —— 只扫 toolchain: 行看不见它", () => {
  // 评审实测的反例：`uses: dtolnay/rust-toolchain@stable` 不带任何输入，旧口径
  // （「每一条 toolchain 都要等于 toml 的 channel」）对它完全失明，守卫返回 0 错误。
  // 危害当下被 rust-toolchain.toml 兜住（rustup 在仓库内覆盖 default），但那是运气。
  const src = [
    "      - uses: dtolnay/rust-toolchain@master",
    "        with:",
    "          toolchain: 1.96.0",
    "      - uses: dtolnay/rust-toolchain@stable",
  ].join("\n");
  assert.throws(() => extractWorkflowToolchains(src, "样例"), /却只有 1 条/);
});

test("三处能从真实源码解析出同一个版本号", () => {
  const channel = extractToolchainChannel(real.tomlSrc);
  assert.match(channel, /^\d+\.\d+/);
  for (const src of [real.ciSrc, real.releaseSrc]) {
    for (const v of extractWorkflowToolchains(src, "真实 workflow")) {
      assert.equal(v, channel);
    }
  }
});
