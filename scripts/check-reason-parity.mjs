#!/usr/bin/env node
// Rust ReasonCode/Confidence ↔ 前端渲染链路的机械一致性校验。
// tsc 只能保证 zh↔en 互相对齐，保证不了 Rust 枚举 ↔ 字典 ↔ 渲染归类对齐。
//
// 校验闭环（评审发现：旧版只查 reason.*/reasonTip.*/confidence.*，而 confidence.*
// 是死键，真正动态渲染的 story.${primary} / verdict.${confidence} 反而无守卫 ——
// 新增 ReasonCode 时行内故事会渲染成裸键名）：
//   1. 每个 ReasonCode：reason.* + reasonTip.* 必须 zh+en 双语齐全（详情面板）；
//   2. 每个 ReasonCode 必须出现在 src/model.ts 的 REASON_PRIORITY（正向码）或
//      EXEMPT_REASONS（豁免码）之一 —— 强制新码归类；
//   3. REASON_PRIORITY / EXEMPT_REASONS 不得引用枚举里不存在的码（防前端陈旧）；
//   4. 每个 REASON_PRIORITY 码：story.* 必须 zh+en 双语齐全（行内故事）；
//   5. 每个 Confidence（除 none）：verdict.* 必须 zh+en 双语齐全（行内判定前缀）；
//   6. Raycast 词表 REASON_LABEL（短语）与 REASON_TIP（解释）——
//      integrations/raycast/src/search-ports.tsx —— 各自与枚举**集合相等**：
//      漏码（新码没配文案）与陈旧码（枚举已删、词表还留）都翻红 —— 该扩展
//      英文单语、不走 src/i18n.ts，这两张表是它唯一被认可的码名消费点，
//      本条守卫是它们被允许存在的前提。
//
// 用法：node scripts/check-reason-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs          （check-reason-parity.test.mjs）

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/**
 * 复刻 serde 的 `RenameRule::SnakeCase`：**每个**非首位大写字母前插下划线。
 *
 * 不能写成 `([a-z0-9])([A-Z])` 那种「小写后接大写」的常见写法 —— 它在连续大写
 * 上会退化：serde 把 `TTYOrphaned` 序列化成 `t_t_y_orphaned`，那个正则却给出
 * `ttyorphaned`。守卫于是拿一个引擎永不产出的键去查字典：字典里有真键、它查的
 * 假键缺失 ⇒ 报一个不存在的错；反之若两边都缺 ⇒ 静默放行。当前 ReasonCode 里
 * 尚无连续大写变体（`Ppid1Orphan` 两种写法同解），这是给未来加变体那天备的。
 */
const camelToSnake = (s) => s.replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase();

/**
 * 提取 `pub enum <name> { ... }` 块内的变体名。
 *
 * 严格解析（评审发现：旧版对「认不出的行」静默跳过 —— 末位变体不带尾逗号、
 * 带负载 `Variant(Foo)`、显式判别值 `Variant = 3` 都会被无声漏掉，恰是
 * 「守卫静默放行比没有守卫更危险」）：块内每一行必须是 空行 / 注释 /
 * `#[...]` 属性 / 无负载变体（尾逗号可选）之一，否则响亮报错。
 * ReasonCode 必须保持无负载形态 —— serde snake_case 键名推导依赖它。
 */
function extractEnumVariants(source, enumName) {
  const start = source.indexOf(`pub enum ${enumName}`);
  if (start === -1) throw new Error(`enum ${enumName} not found in classify.rs`);
  const body = source.slice(start);
  const end = body.indexOf("\n}");
  if (end === -1) throw new Error(`enum ${enumName}: closing brace not found`);
  const block = body.slice(0, end);
  const variants = [];
  for (const rawLine of block.split("\n")) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("//")) continue; // 空行 / 注释（含 ///）
    if (/^#\[[^\]]*\]$/.test(line)) {
      // 属性整体跳过，但 `rename` 必须响亮失败：本守卫按变体名推导 snake_case
      // wire 键，`#[serde(rename = "k8s_managed")]` 会让推导出的 `kubernetes_managed`
      // 与引擎实际发出的键分道扬镳 —— i18n 补齐了推导出的那个键，守卫照样全绿，
      // 而 UI 渲染的是裸键。与 check-model-parity 的 enum 解析同一口径（评审发现：
      // 同一个仓库两个守卫对同一条 serde 语义两种态度）。
      // `rename = "x"` 与 `rename(serialize = "x")` 都会改名，一律拒绝
      if (/\brename\b/.test(line)) {
        throw new Error(
          `enum ${enumName}: 变体带 ${line} —— 本守卫按变体名推导 snake_case wire 键，` +
            "不实现任何 rename 形态；请先在这里实现该语义再加那条属性",
        );
      }
      continue;
    }
    if (/^pub enum [A-Za-z0-9_]+ \{$/.test(line)) continue; // 块首行
    const m = line.match(/^([A-Z][A-Za-z0-9]*)\s*,?\s*(?:\/\/.*)?$/);
    if (m) {
      variants.push(m[1]);
      continue;
    }
    throw new Error(
      `enum ${enumName}: unrecognized line "${line}" — ` +
        `变体必须是无负载形态（serde snake_case 键名推导依赖它），守卫拒绝静默跳过`,
    );
  }
  if (variants.length === 0) throw new Error(`no variants parsed for ${enumName}`);
  return variants;
}

/** 提取 src/model.ts 中 `const NAME = [...]` / `new Set([...])` 字面量里的蛇形码。 */
function extractCodeList(source, constName) {
  const m = source.match(new RegExp(`const ${constName}[^=]*=[^\\[]*\\[([^\\]]*)\\]`, "s"));
  if (!m) throw new Error(`const ${constName} not found in src/model.ts`);
  const codes = [...m[1].matchAll(/"([a-z0-9_]+)"/g)].map((x) => x[1]);
  if (codes.length === 0) throw new Error(`no codes parsed for ${constName}`);
  return codes;
}

/**
 * 提取 Raycast 词表 `const <name> … = {…}` 的键集合（REASON_LABEL / REASON_TIP）。
 *
 * 与 extractEnumVariants 同一态度：块内每一行必须是 空行 / 注释 / `键: 值` /
 * 值的续行（oxfmt 会把超宽的 `键: "长文案"` 折成两行，续行是裸字符串字面量）
 * 之一，否则响亮报错 —— spread、计算键都会让「按行认键」静默漏检，守卫拒绝
 * 在那种形态下假装自己还在工作。键可带引号也可不带（oxfmt 的 as-needed
 * 引号策略两种都会产出）。
 */
function extractRaycastTable(source, constName) {
  const m = source.match(new RegExp(`const ${constName}[^=]*=\\s*\\{([^}]*)\\}`, "s"));
  if (!m) {
    throw new Error(`const ${constName} not found in integrations/raycast/src/search-ports.tsx`);
  }
  const keys = [];
  for (const rawLine of m[1].split("\n")) {
    const line = rawLine.trim();
    if (line === "") continue;
    if (line.startsWith("//") || line.startsWith("/*") || line.startsWith("*")) continue;
    if (/^"[^"]*",?$/.test(line)) continue; // oxfmt 折行后的纯字符串续行
    const km = line.match(/^"?([a-z0-9_]+)"?\s*:/);
    if (km) {
      keys.push(km[1]);
      continue;
    }
    throw new Error(
      `${constName}: unrecognized line "${line}" — ` +
        '词表必须保持「键: "文案"」的平铺字面量，守卫拒绝静默跳过',
    );
  }
  if (keys.length === 0) throw new Error(`no keys parsed for ${constName}`);
  return keys;
}

/**
 * 核心校验（纯函数，可测）：传入四份源码文本，返回错误信息数组（空 = 通过）。
 */
export function checkParity({ classifySrc, i18nSrc, modelSrc, raycastSrc }) {
  const errors = [];
  const reasonCodes = extractEnumVariants(classifySrc, "ReasonCode").map(camelToSnake);
  const confidences = extractEnumVariants(classifySrc, "Confidence")
    .map((v) => v.toLowerCase())
    .filter((v) => v !== "none"); // "none" 不展示，无需文案
  const priority = extractCodeList(modelSrc, "REASON_PRIORITY");
  const exempt = extractCodeList(modelSrc, "EXEMPT_REASONS");

  // 注释行不计入键计数（评审发现：注释里出现 "story.xxx": 字样会伪满足配额）
  const codeLines = i18nSrc.split("\n").filter((l) => !l.trim().startsWith("//"));

  /** key 必须在 zh 与 en 两份字典中各出现一次（非注释行中出现 ≥2 次） */
  function requireKey(key, why) {
    const needle = `"${key}":`;
    let count = 0;
    for (const line of codeLines) count += line.split(needle).length - 1;
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
        `ReasonCode "${code}" 不在 src/model.ts 的 REASON_PRIORITY 也不在 EXEMPT_REASONS —— ` +
          `新增码必须归类：正向码加入 REASON_PRIORITY（并补 story.* 双语键），豁免码加入 EXEMPT_REASONS`,
      );
    }
  }
  for (const code of [...priority, ...exempt]) {
    if (!reasonCodes.includes(code)) {
      errors.push(
        `src/model.ts 引用了枚举中不存在的码 "${code}"（REASON_PRIORITY/EXEMPT_REASONS 陈旧）`,
      );
    }
  }
  for (const code of priority) {
    requireKey(`story.${code}`, "行内主判定故事（story.${primary} 动态渲染）");
  }
  for (const tier of confidences) {
    requireKey(`verdict.${tier}`, "行内判定前缀（verdict.${confidence} 动态渲染）");
  }

  // —— 6. Raycast 词表集合相等（REASON_LABEL 短语 + REASON_TIP 解释）——
  for (const table of ["REASON_LABEL", "REASON_TIP"]) {
    const keys = extractRaycastTable(raycastSrc, table);
    for (const code of reasonCodes) {
      if (!keys.includes(code)) {
        errors.push(
          `Raycast ${table} 缺少 "${code}" —— 新增 ReasonCode 必须在 ` +
            "integrations/raycast/src/search-ports.tsx 配一句普通用户能读的英文文案",
        );
      }
    }
    for (const key of keys) {
      if (!reasonCodes.includes(key)) {
        errors.push(`Raycast ${table} 含枚举中不存在的码 "${key}"（词表陈旧）`);
      }
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一（评审发现，理由同 check-release-assets.mjs）：
// symlink 调用下裸比较不相等 → 守卫静默 exit 0，比没有守卫更糟。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = checkParity({
    classifySrc: readFileSync(join(root, "crates/portreaper-core/src/scanner/classify.rs"), "utf8"),
    i18nSrc: readFileSync(join(root, "src/i18n.ts"), "utf8"),
    modelSrc: readFileSync(join(root, "src/model.ts"), "utf8"),
    raycastSrc: readFileSync(join(root, "integrations/raycast/src/search-ports.tsx"), "utf8"),
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(
      `\nReasonCode/Confidence ↔ i18n/渲染链路 parity check FAILED (${errors.length}).`,
    );
    process.exit(1);
  }
  console.log(
    "✓ reason parity OK — reason/reasonTip/story/verdict 双语齐全、归类闭环，Raycast 词表与枚举集合相等",
  );
}
