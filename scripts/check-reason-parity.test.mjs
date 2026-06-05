// check-reason-parity.mjs 的自测（node --test scripts/*.test.mjs）。
// 守卫脚本自己也是代码：每条校验规则用「真实源码 + 定向突变」验证
// 能抓住对应的回归 —— 否则守卫静默放行比没有守卫更危险（评审发现：
// 旧版脚本校验的 confidence.* 是死键，真正渲染的 story.*/verdict.* 无守卫）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkParity } from "./check-reason-parity.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  classifySrc: readFileSync(join(root, "src-tauri/src/scanner/classify.rs"), "utf8"),
  i18nSrc: readFileSync(join(root, "src/i18n.ts"), "utf8"),
  appSrc: readFileSync(join(root, "src/App.tsx"), "utf8"),
};

test("真实源码当前必须通过全部校验", () => {
  assert.deepEqual(checkParity(real), []);
});

test("Rust 新增未归类的 ReasonCode 必须被拦截", () => {
  const classifySrc = real.classifySrc.replace(
    /pub enum ReasonCode \{/,
    "pub enum ReasonCode {\n    BrandNewCode,",
  );
  const errors = checkParity({ ...real, classifySrc });
  // 未归类 + 缺 reason./reasonTip. 双语键，至少 3 条错误
  assert.ok(errors.some((e) => e.includes("brand_new_code") && e.includes("REASON_PRIORITY")));
  assert.ok(errors.some((e) => e.includes("reason.brand_new_code")));
  assert.ok(errors.some((e) => e.includes("reasonTip.brand_new_code")));
});

test("正向码缺 story.* 键必须被拦截（旧版守卫的盲区）", () => {
  // 把 zh 字典里的 story.defunct 改名，模拟「Rust 加了码、story 漏翻」
  const i18nSrc = real.i18nSrc.replace('"story.defunct":', '"story.defunct_renamed":');
  const errors = checkParity({ ...real, i18nSrc });
  assert.ok(errors.some((e) => e.includes('story.defunct"') && e.includes("only in one language")));
});

test("verdict.* 缺键必须被拦截（旧版守卫的盲区）", () => {
  const i18nSrc = real.i18nSrc.replace('"verdict.likely":', '"verdict.likely_renamed":');
  const errors = checkParity({ ...real, i18nSrc });
  assert.ok(errors.some((e) => e.includes('verdict.likely"')));
});

test("App.tsx 引用枚举不存在的码必须被拦截（前端陈旧）", () => {
  const appSrc = real.appSrc.replace('"defunct",', '"defunct",\n  "ghost_code",');
  const errors = checkParity({ ...real, appSrc });
  assert.ok(errors.some((e) => e.includes('"ghost_code"') && e.includes("不存在")));
});

test("REASON_PRIORITY 漏列正向码必须被拦截", () => {
  const appSrc = real.appSrc.replace('  "defunct",\n', "");
  const errors = checkParity({ ...real, appSrc });
  assert.ok(errors.some((e) => e.includes('"defunct"') && e.includes("REASON_PRIORITY")));
});

test("枚举变体带行内注释仍能被解析（旧正则的静默漏检面）", () => {
  const classifySrc = real.classifySrc.replace(
    /^(\s+)Defunct,\s*$/m,
    "$1Defunct, // inline comment",
  );
  // 解析未漏 Defunct ⇒ 校验结果与基线一致（仍为空）
  assert.deepEqual(checkParity({ ...real, classifySrc }), []);
});
