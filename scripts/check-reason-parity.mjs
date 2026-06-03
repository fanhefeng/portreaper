#!/usr/bin/env node
// Rust ReasonCode/Confidence ↔ 前端 i18n 字典的机械一致性校验。
// tsc 只能保证 zh↔en 互相对齐，保证不了 Rust 枚举 ↔ 字典对齐 ——
// Rust 新增一个 ReasonCode 而前端漏翻，运行时是空 chip 而非编译错误，靠本脚本在 CI 拦截。
//
// 用法：node scripts/check-reason-parity.mjs   （exit 1 = 不一致）

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const classifyRs = readFileSync(
  join(root, "src-tauri/src/scanner/classify.rs"),
  "utf8",
);
const i18nTs = readFileSync(join(root, "src/i18n.ts"), "utf8");

const camelToSnake = (s) =>
  s.replace(/([a-z0-9])([A-Z])/g, "$1_$2").toLowerCase();

/** 提取 `pub enum <name> { ... }` 块内的变体名（serde rename_all 蛇形小写） */
function extractEnumVariants(source, enumName) {
  const start = source.indexOf(`pub enum ${enumName}`);
  if (start === -1) throw new Error(`enum ${enumName} not found in classify.rs`);
  const body = source.slice(start);
  const end = body.indexOf("\n}");
  const block = body.slice(0, end);
  const variants = [];
  for (const line of block.split("\n")) {
    const m = line.match(/^\s{4}([A-Z][A-Za-z0-9]*),\s*$/);
    if (m) variants.push(m[1]);
  }
  if (variants.length === 0) throw new Error(`no variants parsed for ${enumName}`);
  return variants;
}

const reasonCodes = extractEnumVariants(classifyRs, "ReasonCode").map(camelToSnake);
const confidences = extractEnumVariants(classifyRs, "Confidence")
  .map((v) => v.toLowerCase())
  .filter((v) => v !== "none"); // "none" 不展示，无需文案

let failed = false;

/** key 必须在 zh 与 en 两份字典中各出现一次（文件中出现 ≥2 次） */
function requireKey(key) {
  const needle = `"${key}":`;
  const count = i18nTs.split(needle).length - 1;
  if (count < 2) {
    console.error(
      `✗ i18n key "${key}" ${count === 0 ? "missing" : "only in one language"} (found ${count}, need 2: zh + en)`,
    );
    failed = true;
  }
}

for (const code of reasonCodes) {
  requireKey(`reason.${code}`);
  requireKey(`reasonTip.${code}`);
}
for (const tier of confidences) {
  requireKey(`confidence.${tier}`);
}

if (failed) {
  console.error(
    `\nReasonCode/Confidence ↔ i18n parity check FAILED.\n` +
      `Rust codes: ${reasonCodes.join(", ")}\n` +
      `Add the missing keys to BOTH zh and en in src/i18n.ts.`,
  );
  process.exit(1);
}

console.log(
  `✓ reason parity OK — ${reasonCodes.length} reason codes × (label+tip) + ${confidences.length} confidence tiers present in zh & en`,
);
