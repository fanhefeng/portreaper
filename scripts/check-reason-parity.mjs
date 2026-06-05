#!/usr/bin/env node
// Rust ReasonCode/Confidence ↔ 前端渲染链路的机械一致性校验。
// tsc 只能保证 zh↔en 互相对齐，保证不了 Rust 枚举 ↔ 字典 ↔ App.tsx 渲染对齐。
//
// 校验闭环（评审发现：旧版只查 reason.*/reasonTip.*/confidence.*，而 confidence.*
// 是死键，真正动态渲染的 story.${primary} / verdict.${confidence} 反而无守卫 ——
// 新增 ReasonCode 时行内故事会渲染成裸键名）：
//   1. 每个 ReasonCode：reason.* + reasonTip.* 必须 zh+en 双语齐全（详情面板）；
//   2. 每个 ReasonCode 必须出现在 App.tsx 的 REASON_PRIORITY（正向码）或
//      EXEMPT_REASONS（豁免码）之一 —— 强制新码归类；
//   3. REASON_PRIORITY / EXEMPT_REASONS 不得引用枚举里不存在的码（防前端陈旧）；
//   4. 每个 REASON_PRIORITY 码：story.* 必须 zh+en 双语齐全（行内故事）；
//   5. 每个 Confidence（除 none）：verdict.* 必须 zh+en 双语齐全（行内判定前缀）。
//
// 用法：node scripts/check-reason-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs          （check-reason-parity.test.mjs）

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const camelToSnake = (s) =>
  s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();

/** 提取 `pub enum <name> { ... }` 块内的变体名（容忍行内 // 注释）。 */
function extractEnumVariants(source, enumName) {
  const start = source.indexOf(`pub enum ${enumName}`);
  if (start === -1) throw new Error(`enum ${enumName} not found in classify.rs`);
  const body = source.slice(start);
  const end = body.indexOf("\n}");
  const block = body.slice(0, end);
  const variants = [];
  for (const line of block.split("\n")) {
    const m = line.match(/^\s+([A-Z][A-Za-z0-9]*),\s*(?:\/\/.*)?$/);
    if (m) variants.push(m[1]);
  }
  if (variants.length === 0) throw new Error(`no variants parsed for ${enumName}`);
  return variants;
}

/** 提取 App.tsx 中 `const NAME = [...]` / `new Set([...])` 字面量里的蛇形码。 */
function extractCodeList(source, constName) {
  const m = source.match(new RegExp(`const ${constName}[^=]*=[^\\[]*\\[([^\\]]*)\\]`, "s"));
  if (!m) throw new Error(`const ${constName} not found in App.tsx`);
  const codes = [...m[1].matchAll(/"([a-z0-9_]+)"/g)].map((x) => x[1]);
  if (codes.length === 0) throw new Error(`no codes parsed for ${constName}`);
  return codes;
}

/**
 * 核心校验（纯函数，可测）：传入三份源码文本，返回错误信息数组（空 = 通过）。
 */
export function checkParity({ classifySrc, i18nSrc, appSrc }) {
  const errors = [];
  const reasonCodes = extractEnumVariants(classifySrc, "ReasonCode").map(camelToSnake);
  const confidences = extractEnumVariants(classifySrc, "Confidence")
    .map((v) => v.toLowerCase())
    .filter((v) => v !== "none"); // "none" 不展示，无需文案
  const priority = extractCodeList(appSrc, "REASON_PRIORITY");
  const exempt = extractCodeList(appSrc, "EXEMPT_REASONS");

  /** key 必须在 zh 与 en 两份字典中各出现一次（文件中出现 ≥2 次） */
  function requireKey(key, why) {
    const needle = `"${key}":`;
    const count = i18nSrc.split(needle).length - 1;
    if (count < 2) {
      errors.push(
        `i18n key "${key}" ${count === 0 ? "missing" : "only in one language"} (found ${count}, need 2: zh + en) — ${why}`,
      );
    }
  }

  for (const code of reasonCodes) {
    requireKey(`reason.${code}`, "详情面板短标签");
    requireKey(`reasonTip.${code}`, "详情面板完整解释");
    if (!priority.includes(code) && !exempt.includes(code)) {
      errors.push(
        `ReasonCode "${code}" 不在 App.tsx 的 REASON_PRIORITY 也不在 EXEMPT_REASONS —— ` +
          `新增码必须归类：正向码加入 REASON_PRIORITY（并补 story.* 双语键），豁免码加入 EXEMPT_REASONS`,
      );
    }
  }
  for (const code of [...priority, ...exempt]) {
    if (!reasonCodes.includes(code)) {
      errors.push(`App.tsx 引用了枚举中不存在的码 "${code}"（REASON_PRIORITY/EXEMPT_REASONS 陈旧）`);
    }
  }
  for (const code of priority) {
    requireKey(`story.${code}`, "行内主判定故事（story.${primary} 动态渲染）");
  }
  for (const tier of confidences) {
    requireKey(`verdict.${tier}`, "行内判定前缀（verdict.${confidence} 动态渲染）");
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = checkParity({
    classifySrc: readFileSync(join(root, "src-tauri/src/scanner/classify.rs"), "utf8"),
    i18nSrc: readFileSync(join(root, "src/i18n.ts"), "utf8"),
    appSrc: readFileSync(join(root, "src/App.tsx"), "utf8"),
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\nReasonCode/Confidence ↔ i18n/渲染链路 parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log("✓ reason parity OK — reason/reasonTip/story/verdict 渲染链路全部双语齐全且归类闭环");
}
