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

/**
 * 变体名 → serde `rename_all = "snake_case"` 的 wire 名。
 *
 * **逐字复刻 serde_derive 的算法**（`internals/case.rs`：首字符之外每遇一个大写就
 * 先插 `_`，再整体小写），不是「词首大写」那种近似：近似写法 `([a-z0-9])([A-Z])`
 * 对 `IOError` 给出 `ioerror`，而 serde 发出的是 `i_o_error` —— 守卫拿着错的 wire 名
 * 比对通过，等于说了一句假话，比没有守卫更危险（评审发现）。
 * 与 `check-reason-parity.mjs` 的 `camelToSnake` 同一实现。
 */
function pascalToSnake(name) {
  return name.replace(/(?<!^)([A-Z])/g, "_$1").toLowerCase();
}

/** enum 上的容器级 serde 属性：本守卫只支持 `rename_all = "snake_case"`。
 *  换成 kebab-case / camelCase 而守卫照旧按 snake_case 推导，是同一类静默说谎。 */
function assertSnakeCaseContainer(rustSrc, enumName) {
  const at = rustSrc.search(new RegExp(`pub enum ${enumName}\\s*\\{`));
  // 取 enum 声明前紧邻的那段属性块（derive / serde），足以覆盖 rename_all 的写法
  const before = rustSrc.slice(0, at);
  const attrs = before.slice(Math.max(0, before.lastIndexOf("\n\n")));
  const m = attrs.match(/rename_all\s*=\s*"([^"]+)"/);
  if (!m) {
    throw new Error(
      `\`${enumName}\` 没有 #[serde(rename_all = ...)] —— 本守卫按 snake_case 推导 wire 名，` +
        "请确认序列化形态后再更新这里",
    );
  }
  if (m[1] !== "snake_case") {
    throw new Error(
      `\`${enumName}\` 的 rename_all 是 "${m[1]}"，本守卫只实现了 snake_case —— ` +
        "推导出的键会与引擎实际发出的分道扬镳，守卫拒绝猜测",
    );
  }
}

/**
 * 提取 Rust enum 的 serde wire 变体名（假定 `rename_all = "snake_case"`）。
 *
 * 严格解析，三条与 `extractRustFields` 一致的纪律：找不到 enum 抛、解析出 0 个
 * 变体抛、注释行不参与。**遇到 `#[serde(rename = …)]` 直接抛**：本守卫不实现
 * rename 语义，与其按变体名静默放行、让守卫说一句假话，不如逼人来改这里
 * （check-reason-parity 忽略 rename 正是同类漏洞）。
 */
export function extractRustEnumVariants(rustSrc, enumName) {
  const open = rustSrc.match(new RegExp(`pub enum ${enumName}\\s*\\{`));
  if (!open) {
    throw new Error(
      `找不到 \`pub enum ${enumName}\` —— enum 被改名或挪走时，本守卫必须响亮失败而不是放行`,
    );
  }
  assertSnakeCaseContainer(rustSrc, enumName);
  const body = rustSrc.slice(open.index + open[0].length);
  const variants = [];
  let depth = 0;
  for (const line of body.split("\n")) {
    const t = line.trim();
    if (depth === 0 && t === "}") break;
    if (!t.startsWith("//") && depth === 0) {
      // 属性行按**前缀**识别，不去解析它的括号结构：`#[serde(rename(serialize = "x"))]`
      // 里的嵌套括号会让 `\(([^)]*)\)\]` 整条匹配失败，于是那一行既不算属性、也不算
      // 变体，被静默跳过 —— 正是本仓库反复强调的「守卫静默放行比没有守卫更危险」。
      if (t.startsWith("#[")) {
        // `rename = "x"` 与 `rename(serialize = "x")` 都会改变 wire 名，一律拒绝
        if (/\brename\b/.test(t)) {
          throw new Error(
            `\`${enumName}\` 的变体带 ${t} —— 本守卫按 snake_case 推导 wire 名，` +
              "不实现任何 rename 形态；请先在这里实现该语义再加那条属性",
          );
        }
      } else {
        // 变体名：PascalCase 起头，后接 `,`（单元）/`{`（结构体）/`(`（元组）
        const v = t.match(/^([A-Z][A-Za-z0-9]*)\s*[,{(]/);
        if (v) variants.push(pascalToSnake(v[1]));
      }
    }
    depth += (t.match(/\{/g) ?? []).length - (t.match(/\}/g) ?? []).length;
  }
  if (variants.length === 0) {
    throw new Error(`\`pub enum ${enumName}\` 解析出 0 个变体 —— 解析器或 enum 形态已失配`);
  }
  return variants;
}

/**
 * 提取 TS 联合类型里的 snake_case 字符串字面量。
 * 桌面侧是 `{ code: "process_gone" }` 形态，Raycast 侧是裸 `"process_gone"` 形态，
 * 两者都只需把字面量捞出来 —— 比对的是**码集合**，不是类型的结构。
 */
export function extractTsUnionLiterals(tsSrc, typeName, label) {
  const noBlockComments = tsSrc.replace(/\/\*[\s\S]*?\*\//g, "");
  const open = noBlockComments.match(new RegExp(`export type ${typeName}\\s*=`));
  if (!open) {
    throw new Error(
      `在 ${label} 里找不到 \`export type ${typeName}\` —— ` +
        "类型被改名或挪走时，本守卫必须响亮失败而不是放行",
    );
  }
  const rest = noBlockComments.slice(open.index + open[0].length);
  const end = rest.indexOf(";");
  const body = end === -1 ? rest : rest.slice(0, end);
  const literals = [...body.matchAll(/"([a-z0-9_]+)"/g)].map((m) => m[1]);
  if (literals.length === 0) {
    throw new Error(`${label} 的 \`${typeName}\` 解析出 0 个字面量 —— 解析器或类型形态已失配`);
  }
  return literals;
}

/**
 * `KillError` 的 wire 码三方一致性。
 *
 * 为什么必须有：CLAUDE.md 把「新增变体要同时改三处」写成硬要求，但此前**没有任何
 * 东西在守**。漏改整条链路时三层门禁一条都不响 —— Rust 侧 `Display` 的穷尽 match
 * 只逼你改 Display；桌面 `asKillError` 与 Raycast `parseKillError` 各有一条
 * `default: return null` 把陌生码吃掉，tsc 全绿。所谓「union member with no switch
 * arm is a tsc error」只在**有人已经改了 TS 联合**时才成立。`os` 变体当初就是这么
 * 在 Raycast 侧长期缺失的（评审发现）。
 */
export function checkKillErrorParity({ platformSrc, desktopSrc, raycastSrc }) {
  const rust = extractRustEnumVariants(platformSrc, "KillError");
  const errors = [];
  for (const [label, literals] of [
    ["src/model.ts", extractTsUnionLiterals(desktopSrc, "KillError", "src/model.ts")],
    [
      "integrations/raycast/src/cli.ts",
      extractTsUnionLiterals(raycastSrc, "KillErrorCode", "integrations/raycast/src/cli.ts"),
    ],
  ]) {
    const diff = diffFields(rust, literals, "码");
    if (diff !== "") {
      errors.push(
        `KillError 码失配：${label} ${diff} —— ` +
          "陌生码会被前端的 default 分支静默吃掉，用户拿到的是一条没有语义的失败",
      );
    }
  }
  return errors;
}

/** 两个集合的差异描述（空串 = 一致）。以 Rust wire 名为基准方向表述。 */
function diffFields(rustFields, mirrorFields, noun = "字段") {
  const rust = new Set(rustFields);
  const mirror = new Set(mirrorFields);
  const missing = [...rust].filter((f) => !mirror.has(f)).sort();
  const extra = [...mirror].filter((f) => !rust.has(f)).sort();
  const parts = [];
  if (missing.length > 0) parts.push(`缺少引擎${noun} [${missing.join(", ")}]`);
  if (extra.length > 0) parts.push(`多出引擎没有的${noun} [${extra.join(", ")}]`);
  return parts.join("；");
}

/**
 * 「同一个进程」的身份容差，三处必须同值。
 *
 * 引擎的 `START_TOLERANCE_SECS`（platform.rs）用于 kill 前的 PID 复用防护；
 * 两个前端的 `START_MATCH_TOLERANCE_SECS` 用于终止后的存活确认。三处注释互相
 * 声明「取值一致」，但在此之前**没有任何东西拦得住把它从 5 改成 10** —— 而放宽
 * 这个容差的后果正是这套代码要防的那一类：PID 被复用后仍被认成同一个进程
 * （评审发现）。抽成常量集中比对，分叉即 CI 变红。
 *
 * 顺带解释为什么它必须有容差、不能用严格相等：`start_unix` 由 `now - etime`
 * 推导，而 `etime` 只有秒级粒度 —— 同一个进程在连续两轮扫描里读到的值会
 * ±1s 抖动（实测 14 轮采样、13 个进程全部出现 1 秒极差）。
 */
/**
 * 容差的**期望取值**，与三处源码一起钉死。
 *
 * 只校验「三处相等」是不够的：三处一起从 5 改成 10 会照常通过，而
 * `CLAUDE.md` 与 `.coderabbit.yaml` 里白纸黑字写的是「±5s」—— 文档会就此
 * 静默变成假话，且身份匹配窗口被悄悄放宽（评审发现）。
 *
 * 这个数不是物理常量，是留了余量的工程取值（实测抖动 ±1~2s，取 5）。真要改它
 * 是一次**需要被评审**的决定：改这里、改三处源码、改 CLAUDE.md 与
 * .coderabbit.yaml 里点名 ±5s 的那两句 —— 守卫的作用正是逼出这一步，
 * 而不是让它顺手滑过去。
 */
const EXPECTED_TOLERANCE_SECS = 5;

const TOLERANCE_SITES = [
  {
    label: "crates/portreaper-core/src/platform.rs",
    key: "platformSrc",
    re: /const\s+START_TOLERANCE_SECS\s*:\s*u64\s*=\s*(\d+)\s*;/,
  },
  {
    label: "src/model.ts",
    key: "desktopSrc",
    re: /export\s+const\s+START_MATCH_TOLERANCE_SECS\s*=\s*(\d+)\s*;/,
  },
  {
    label: "integrations/raycast/src/cli.ts",
    key: "raycastSrc",
    re: /export\s+const\s+START_MATCH_TOLERANCE_SECS\s*=\s*(\d+)\s*;/,
  },
];

/** 三处身份容差的取值比对；返回错误信息数组（空 = 通过）。 */
export function checkToleranceParity(sources) {
  const found = [];
  for (const site of TOLERANCE_SITES) {
    const m = (sources[site.key] ?? "").match(site.re);
    if (!m) {
      // 常量被改名/挪走时必须响亮失败 —— 「没找到 = 没问题」比没有守卫更糟
      return [`${site.label} 里找不到身份容差常量 —— 改名或挪走时守卫必须失败而不是放行`];
    }
    found.push({ label: site.label, value: Number(m[1]) });
  }
  const where = found.map((f) => `${f.label}=${f.value}`).join("，");
  const values = new Set(found.map((f) => f.value));
  if (values.size > 1) {
    return [`身份容差三处不一致：${where} —— 放宽它会让被复用的 PID 被认成同一个进程`];
  }
  if (found[0].value !== EXPECTED_TOLERANCE_SECS) {
    return [
      `身份容差必须是 ${EXPECTED_TOLERANCE_SECS} 秒，实际 ${where} —— ` +
        "CLAUDE.md 与 .coderabbit.yaml 都点名了 ±5s；真要改请连同这个守卫与那两处文档一起改",
    ];
  }
  return [];
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
  const desktopSrc = readFileSync(join(root, "src/model.ts"), "utf8");
  const raycastSrc = readFileSync(join(root, "integrations/raycast/src/cli.ts"), "utf8");
  const platformSrc = readFileSync(join(root, "crates/portreaper-core/src/platform.rs"), "utf8");
  const errors = [
    ...checkModelParity({
      rustSrc: readFileSync(join(root, "crates/portreaper-core/src/scanner/model.rs"), "utf8"),
      desktopSrc,
      raycastSrc,
    }),
    ...checkToleranceParity({ platformSrc, desktopSrc, raycastSrc }),
    ...checkKillErrorParity({ platformSrc, desktopSrc, raycastSrc }),
  ];
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\nmodel parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log(
    "✓ model parity OK — ProcessEntry/ParentRef 字段集、KillError 码集与身份容差三处一致",
  );
}
