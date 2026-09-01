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
  classifySrc: readFileSync(join(root, "crates/portreaper-core/src/scanner/classify.rs"), "utf8"),
  i18nSrc: readFileSync(join(root, "src/i18n.ts"), "utf8"),
  modelSrc: readFileSync(join(root, "src/model.ts"), "utf8"),
  raycastSrc: readFileSync(join(root, "integrations/raycast/src/search-ports.tsx"), "utf8"),
};

test("真实源码当前必须通过全部校验", () => {
  assert.deepEqual(checkParity(real), []);
});

test("变体带 #[serde(rename)] 时响亮失败 —— 守卫按变体名推导键，不实现 rename", () => {
  // 评审实测过的反例：加一个 rename 到别的 wire 名，再照守卫的提示把推导出的
  // 那个键在 i18n 与 EXEMPT_REASONS 里补齐 —— 旧版守卫返回 0 个错误，而引擎实际
  // 发出的是 `k8s_managed`，UI 渲染裸键。守卫必须拒绝猜测，逼人来实现该语义。
  const classifySrc = real.classifySrc.replace(
    /pub enum ReasonCode \{/,
    'pub enum ReasonCode {\n    #[serde(rename = "k8s_managed")]\n    KubernetesManaged,',
  );
  assert.notEqual(classifySrc, real.classifySrc, "突变未生效：enum 声明形态已变");
  assert.throws(() => checkParity({ ...real, classifySrc }), /rename/);
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

test("model.ts 引用枚举不存在的码必须被拦截（前端陈旧）", () => {
  const modelSrc = real.modelSrc.replace('"defunct",', '"defunct",\n  "ghost_code",');
  const errors = checkParity({ ...real, modelSrc });
  assert.ok(errors.some((e) => e.includes('"ghost_code"') && e.includes("不存在")));
});

test("REASON_PRIORITY 漏列正向码必须被拦截", () => {
  const modelSrc = real.modelSrc.replace('  "defunct",\n', "");
  const errors = checkParity({ ...real, modelSrc });
  assert.ok(errors.some((e) => e.includes('"defunct"') && e.includes("REASON_PRIORITY")));
});

test("枚举变体带行内注释仍能被解析（旧正则的静默漏检面）", () => {
  const classifySrc = real.classifySrc.replace(
    /^(\s+)Defunct,\s*$/m,
    "$1Defunct, // inline comment",
  );
  assert.notEqual(classifySrc, real.classifySrc, "突变未生效：Defunct 的行形态已变，用例形同虚设");
  // 解析未漏 Defunct ⇒ 校验结果与基线一致（仍为空）
  assert.deepEqual(checkParity({ ...real, classifySrc }), []);
});

test("末位变体不带尾逗号仍能被解析（旧正则的静默漏检面）", () => {
  // 把 JustReparented 的尾逗号去掉（手写常见形态，rustfmt 跑过才会补回）
  const classifySrc = real.classifySrc.replace(/JustReparented,/, "JustReparented");
  assert.notEqual(classifySrc, real.classifySrc, "突变未生效：JustReparented 已改名，用例形同虚设");
  assert.deepEqual(checkParity({ ...real, classifySrc }), []);
});

test("连续大写变体按 serde 规则转键（每个大写字母前都断词）", () => {
  // serde 的 SnakeCase 对 TTYOrphaned 产出 t_t_y_orphaned；「小写后接大写」那类
  // 正则会给出 ttyorphaned —— 守卫据此去查字典就是在查一个引擎永不产出的键
  const classifySrc = real.classifySrc.replace(
    /pub enum ReasonCode \{/,
    "pub enum ReasonCode {\n    TTYOrphaned,",
  );
  assert.notEqual(classifySrc, real.classifySrc, "突变未生效，用例形同虚设");
  const errors = checkParity({ ...real, classifySrc });
  assert.ok(
    errors.some((e) => e.includes("t_t_y_orphaned")),
    `期望按 serde 规则报 t_t_y_orphaned，实际：${errors.join(" | ")}`,
  );
  assert.ok(!errors.some((e) => e.includes("ttyorphaned")), "不得出现退化键 ttyorphaned");
});

test("带负载 / 显式判别值的变体必须响亮报错而非静默跳过", () => {
  // serde snake_case 键名推导依赖无负载形态 —— 出现负载说明契约被破坏
  const payload = real.classifySrc.replace(
    /pub enum ReasonCode \{/,
    "pub enum ReasonCode {\n    Sneaky(u8),",
  );
  assert.throws(() => checkParity({ ...real, classifySrc: payload }), /unrecognized line/);

  const discriminant = real.classifySrc.replace(
    /pub enum ReasonCode \{/,
    "pub enum ReasonCode {\n    Sneaky = 3,",
  );
  assert.throws(() => checkParity({ ...real, classifySrc: discriminant }), /unrecognized line/);
});

test("Raycast 词表漏码必须被拦截（新码没配可读文案）", () => {
  const raycastSrc = real.raycastSrc.replace(/^\s*defunct: .*\r?\n/m, "");
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效：REASON_LABEL 的 defunct 行形态已变");
  const errors = checkParity({ ...real, raycastSrc });
  assert.ok(errors.some((e) => e.includes('"defunct"') && e.includes("REASON_LABEL")));
});

test("Raycast 词表含陈旧码必须被拦截（枚举已删、词表还留）", () => {
  const raycastSrc = real.raycastSrc.replace(
    /const REASON_LABEL[^=]*= \{/,
    '$&\n  ghost_code: "stale entry",',
  );
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效：REASON_LABEL 声明形态已变");
  const errors = checkParity({ ...real, raycastSrc });
  assert.ok(errors.some((e) => e.includes('"ghost_code"') && e.includes("REASON_LABEL")));
});

test("Raycast 解释表（REASON_TIP）漏码/陈旧码同样被拦截", () => {
  // 把 REASON_TIP 的 defunct 键改名 —— 一次突变同时制造「缺 defunct」与
  // 「多出 defunct_renamed」两种错误，且都必须标明出自 REASON_TIP
  const raycastSrc = real.raycastSrc.replace(
    /(const REASON_TIP[^=]*= \{\s*\n\s*)defunct:/,
    "$1defunct_renamed:",
  );
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效：REASON_TIP 的 defunct 键形态已变");
  const errors = checkParity({ ...real, raycastSrc });
  assert.ok(errors.some((e) => e.includes('"defunct"') && e.includes("REASON_TIP")));
  assert.ok(errors.some((e) => e.includes('"defunct_renamed"') && e.includes("REASON_TIP")));
});

test("Raycast 词表出现认不出的行必须响亮报错而非静默跳过", () => {
  // spread / 跨行值 / 计算键都会让「按行认键」漏检 —— 守卫必须拒绝这种形态
  const raycastSrc = real.raycastSrc.replace(
    /const REASON_LABEL[^=]*= \{/,
    "$&\n  ...spreadFromSomewhere,",
  );
  assert.notEqual(raycastSrc, real.raycastSrc, "突变未生效：REASON_LABEL 声明形态已变");
  assert.throws(() => checkParity({ ...real, raycastSrc }), /REASON_LABEL/);
});

test("注释里的键字样不得伪满足双语配额", () => {
  // 把 zh 的 story.defunct 真键注释掉、再在注释里留一份 —— 计数必须只认非注释行
  const i18nSrc = real.i18nSrc.replace(
    /^(\s*)"story\.defunct":(.*)$/m,
    '$1// "story.defunct":$2\n$1// "story.defunct": "decoy",',
  );
  const errors = checkParity({ ...real, i18nSrc });
  assert.ok(errors.some((e) => e.includes('story.defunct"') && e.includes("only in one language")));
});
