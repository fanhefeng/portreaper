#!/usr/bin/env node
// ProcessEntry / ParentRef 序列化契约的三方镜像一致性校验。
//
// 为什么需要：引擎的 serde 输出（crates/portreaper-core/src/scanner/model.rs，
// 真相源）有两份手写 TS 镜像 —— src/model.ts（桌面前端）与
// integrations/raycast/src/cli.ts（Raycast 前端）。字段增删漏同步的症状不是
// 报错：TS 侧读到 undefined，渲染成空串 / NaN，或新字段静默不显示 —— 一个
// 只让人怀疑「是不是引擎没给」的 bug，必须由守卫拦在提交前。
//
// 口径：只比**字段名集合**（Rust 侧以 serde wire 名为准 —— rename 认 rename，
// skip 不计入）；形态（u32 vs number）由各自类型系统管，两边名字对上才是契约。
//
// 用法：node scripts/check-model-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** 三方都要镜像的结构体 —— 新增共享结构体时把名字加进这里即可纳管。 */
export const MIRRORED_STRUCTS = ["ProcessEntry", "ParentRef"];

/**
 * 提取 Rust struct 的 serde wire 字段名列表。
 *
 * 严格解析：struct 找不到、或解析出 0 个字段，都直接抛 —— 结构体被改名 /
 * 挪走时守卫必须响亮失败而不是「没找到 = 没问题」（同 check-paths-parity）。
 * 注释行（// 与 ///）不参与解析：注释掉的旧字段不得被当作真值。
 * serde 属性按 wire 语义处理：`rename = "x"` 以 x 计，`skip`/`skip_serializing`
 * 不计入 —— 镜像的对象是 JSON 输出，不是 Rust 内存布局。
 */
export function extractRustFields(rustSrc, structName) {
  const open = rustSrc.match(new RegExp(`pub struct ${structName}\\s*\\{`));
  if (!open) {
    throw new Error(
      `在 model.rs 里找不到 \`pub struct ${structName}\` —— ` +
        "结构体被改名或挪走时，本守卫必须响亮失败而不是放行",
    );
  }
  const body = rustSrc.slice(open.index + open[0].length);
  const fields = [];
  let rename = null;
  let skip = false;
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (t === "}") break;
    if (t.startsWith("//")) continue; // 含 /// 文档注释
    const attr = t.match(/^#\[serde\(([^)]*)\)\]/);
    if (attr) {
      const r = attr[1].match(/rename\s*=\s*"([^"]+)"/);
      if (r) rename = r[1];
      if (/\bskip(_serializing)?\b/.test(attr[1])) skip = true;
      continue;
    }
    const field = t.match(/^pub\s+(?:r#)?([A-Za-z_][A-Za-z0-9_]*)\s*:/);
    if (field) {
      if (!skip) fields.push(rename ?? field[1]);
      rename = null;
      skip = false;
    }
  }
  if (fields.length === 0) {
    throw new Error(`\`pub struct ${structName}\` 解析出 0 个字段 —— 解析器或结构体形态已失配`);
  }
  return fields;
}

/**
 * 提取 TS `export type X = { ... }` 的字段名列表。
 * 先整体剔除块注释（类型体内有多行 JSDoc），再逐行剔除 // 注释行。
 * 找不到类型或 0 字段同样直接抛。
 */
export function extractTsFields(tsSrc, typeName, label) {
  const noBlockComments = tsSrc.replace(/\/\*[\s\S]*?\*\//g, "");
  const open = noBlockComments.match(new RegExp(`export type ${typeName}\\s*=\\s*\\{`));
  if (!open) {
    throw new Error(
      `在 ${label} 里找不到 \`export type ${typeName}\` —— ` +
        "类型被改名或挪走时，本守卫必须响亮失败而不是放行",
    );
  }
  const body = noBlockComments.slice(open.index + open[0].length);
  const fields = [];
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (t.startsWith("};") || t === "}") break;
    if (t.startsWith("//")) continue;
    const field = t.match(/^([A-Za-z_][A-Za-z0-9_]*)\s*\??:/);
    if (field) fields.push(field[1]);
  }
  if (fields.length === 0) {
    throw new Error(`${label} 的 \`${typeName}\` 解析出 0 个字段 —— 解析器或类型形态已失配`);
  }
  return fields;
}

/** 两个集合的差异描述（空串 = 一致）。以 Rust wire 名为基准方向表述。 */
function diffFields(rustFields, mirrorFields) {
  const rust = new Set(rustFields);
  const mirror = new Set(mirrorFields);
  const missing = [...rust].filter((f) => !mirror.has(f)).sort();
  const extra = [...mirror].filter((f) => !rust.has(f)).sort();
  const parts = [];
  if (missing.length > 0) parts.push(`缺少引擎字段 [${missing.join(", ")}]`);
  if (extra.length > 0) parts.push(`多出引擎没有的字段 [${extra.join(", ")}]`);
  return parts.join("；");
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkModelParity({ rustSrc, desktopSrc, raycastSrc }) {
  const errors = [];
  for (const struct of MIRRORED_STRUCTS) {
    const rustFields = extractRustFields(rustSrc, struct);
    for (const [label, src] of [
      ["src/model.ts", desktopSrc],
      ["integrations/raycast/src/cli.ts", raycastSrc],
    ]) {
      const diff = diffFields(rustFields, extractTsFields(src, struct, label));
      if (diff !== "") {
        errors.push(
          `${struct} 镜像失配：${label} ${diff} —— ` +
            "前端会把缺失字段渲染成 undefined/NaN，且不会报任何错",
        );
      }
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一：symlink 调用下裸比较不相等 → 守卫静默 exit 0，比没有守卫更糟。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = checkModelParity({
    rustSrc: readFileSync(join(root, "crates/portreaper-core/src/scanner/model.rs"), "utf8"),
    desktopSrc: readFileSync(join(root, "src/model.ts"), "utf8"),
    raycastSrc: readFileSync(join(root, "integrations/raycast/src/cli.ts"), "utf8"),
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\nmodel parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log("✓ model parity OK — ProcessEntry/ParentRef 的三份契约字段一致");
}
