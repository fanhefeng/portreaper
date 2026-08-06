#!/usr/bin/env node
// bump-version.mjs — keep the app version in lockstep across all manifests.
//
// Usage:
//   node scripts/bump-version.mjs 0.2.0          rewrite the version everywhere
//   node scripts/bump-version.mjs --check 0.2.0  verify everything already agrees
//
// Touches five files (paths are relative to the repo root):
//   package.json                    .version          (JSON)
//   src-tauri/tauri.conf.json       .version          (JSON)
//   src-tauri/Cargo.toml            [package] version (TOML, first match only)
//   crates/portreaper-cli/Cargo.toml [package] version (TOML)
//   Cargo.lock                      [[package]] name="portreaper" / "portreaper-cli"
//
// Cargo.lock 住在仓库根（workspace 根也在那里），而各 Cargo.toml 在各自 crate 下 ——
// 这不是笔误：lockfile 属于整个 workspace，版本号属于各个 crate。
//
// 纳入本脚本的判据是「**用户能不能看见这个版本号**」：
//   - portreaper（桌面应用）与 portreaper-cli 都是**发布产物**（前者是安装包，
//     后者是 release 资产，用户会下载、会在 --version 里读到它、会拿它报 issue）。
//     它们的版本必须能对应到某一个 release，故必须同步。
//   - crates/portreaper-core 是**不发布的内部库**，用户永远看不到它的版本，
//     故刻意不管 —— 让它按自己的节奏走，避免每次发版都产生无意义的 diff。
//
// Zero dependencies. Requires Node >= 18 (ESM, fs/promises).

import { realpathSync } from "node:fs";
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, "..");

const PKG_JSON = join(ROOT, "package.json");
const TAURI_CONF = join(ROOT, "src-tauri", "tauri.conf.json");
const CARGO_TOML = join(ROOT, "src-tauri", "Cargo.toml");
const CLI_CARGO_TOML = join(ROOT, "crates", "portreaper-cli", "Cargo.toml");
const CARGO_LOCK = join(ROOT, "Cargo.lock"); // workspace 根，非 src-tauri/

// semver-ish: major.minor.patch with an optional pre-release/build suffix.
// 每个数字段禁前导零（0 | [1-9]\d*），与 cargo 一致 —— 否则 01.2.3 能写进 JSON
// 清单，却被 cargo 拒绝，造成跨清单半应用的 bump（评审 D1）。
export const SEMVER_RE =
  /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

/** Cargo.lock 里需要同步版本的包 —— 即所有「用户看得见版本号」的发布产物。 */
export const LOCK_CRATE_NAMES = ["portreaper", "portreaper-cli"];

function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

/**
 * 纯改写函数（`setCargo*Version`）的校验失败走 throw，不走 `fail()`。
 *
 * 它们是 export 出去给 node:test 用的：`process.exit(1)` 会把整个测试进程带走，
 * 于是那几条「清单缺 [package] / 缺 version 行必须响亮失败」的分支根本无法被
 * 断言覆盖 —— 唯一能验证它们的手段反而杀死了验证过程。main() 本就有 try/catch
 * 把异常转成 `fail()`，CLI 侧的退出码与错误文案完全不变（评审发现）。
 */
function invalid(msg) {
  throw new Error(msg);
}

function parseArgs(argv) {
  const args = argv.slice(2);
  let check = false;
  const positionals = [];
  for (const a of args) {
    if (a === "--check") check = true;
    else if (a === "--help" || a === "-h") {
      console.log(
        "Usage:\n" +
          "  node scripts/bump-version.mjs <version>\n" +
          "  node scripts/bump-version.mjs --check <version>",
      );
      process.exit(0);
    } else positionals.push(a);
  }
  const version = positionals[0];
  if (version === undefined || version === "") {
    fail("missing version argument (e.g. `node scripts/bump-version.mjs 0.2.0`)");
  }
  if (!SEMVER_RE.test(version)) {
    fail(`"${version}" is not a valid version (expected MAJOR.MINOR.PATCH[-suffix])`);
  }
  return { check, version };
}

// --- JSON helpers: 定点替换，原文件格式逐字节保留。 -------------------------
// 不用 JSON.parse + JSON.stringify 整体重写（评审根因，v0.7.0 发版实锤）：
// 重写会抹掉 oxfmt 的既有风格 —— tauri.conf.json 被折叠的 targets 数组
// 被重新展开，主分支的 vp check 门禁当场变红。与 Cargo.toml/lock 的
// regex 定点手术保持同一纪律：只动 version 的值，其余一个字节不碰。

async function readJsonVersion(path) {
  const raw = await readFile(path, "utf8");
  const data = JSON.parse(raw);
  return data.version;
}

/** 替换首个 `"version": "…"` 的值；失败或未生效返回 null（调用方响亮退出）。 */
export function setJsonVersion(raw, version) {
  const re = /("version"\s*:\s*")[^"]*(")/;
  if (!re.test(raw)) return null;
  const out = raw.replace(re, `$1${version}$2`);
  // 双保险：结果必须仍是合法 JSON，且顶层 version 确已更新 ——
  // 若首个匹配命中的是某个嵌套 "version"，此检查会失败而非静默写坏文件。
  try {
    if (JSON.parse(out).version !== version) return null;
  } catch {
    return null;
  }
  return out;
}

async function writeJsonVersion(path, version) {
  const raw = await readFile(path, "utf8");
  const out = setJsonVersion(raw, version);
  if (out === null) fail(`could not update "version" in ${path}`);
  await writeFile(path, out);
}

// --- Cargo.toml: rewrite the version in the [package] section only. ---------
// We scope the search to the [package] table so a `version = "..."` in some
// other table (e.g. a dependency) is never matched.

export function findCargoTomlVersion(raw) {
  const pkg = sliceTomlSection(raw, "package");
  const m = pkg.match(/^\s*version\s*=\s*"([^"]*)"/m);
  return m ? m[1] : undefined;
}

function sliceTomlSection(raw, section) {
  const header = new RegExp(`^\\[${section}\\]\\s*$`, "m");
  const start = raw.search(header);
  if (start === -1) return "";
  const after = raw.slice(start + raw.match(header)[0].length);
  // Stop at the next top-level table header `[...]` (not an array `[[...]]`
  // continuation — a fresh `[` at line start ends the section either way).
  const next = after.search(/^\[/m);
  return next === -1 ? after : after.slice(0, next);
}

export function setCargoTomlVersion(raw, version) {
  const header = /^\[package\]\s*$/m;
  const start = raw.search(header);
  if (start === -1) invalid("could not find [package] section in Cargo.toml");
  const headerLen = raw.match(header)[0].length;
  const sectionStart = start + headerLen;
  const after = raw.slice(sectionStart);
  const nextTable = after.search(/^\[/m);
  const sectionEnd = nextTable === -1 ? raw.length : sectionStart + nextTable;

  const section = raw.slice(sectionStart, sectionEnd);
  const replaced = section.replace(/^(\s*version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
  if (replaced === section && !/^\s*version\s*=/m.test(section)) {
    invalid('could not find `version = "..."` in [package] section of Cargo.toml');
  }
  return raw.slice(0, sectionStart) + replaced + raw.slice(sectionEnd);
}

// --- Cargo.lock: rewrite the version of the `portreaper` [[package]] block. -

export function findCargoLockVersion(raw, crateName = LOCK_CRATE_NAMES[0]) {
  const block = findCargoLockBlock(raw, crateName);
  if (!block) return undefined;
  const m = block.text.match(/^version\s*=\s*"([^"]*)"/m);
  return m ? m[1] : undefined;
}

function findCargoLockBlock(raw, crateName) {
  // Each entry is `[[package]]\nname = "..."\nversion = "..."\n...`.
  // \r?\n：.gitattributes 已钉 LF，但 Windows 上 autocrlf=true 的旧检出
  // 仍可能是 CRLF —— 容忍它，避免脚本在那种环境响亮失败（评审发现）。
  const re = /\[\[package\]\]\r?\n([\s\S]*?)(?=\r?\n\[\[package\]\]|(?:\r?\n)*$)/g;
  let m;
  while ((m = re.exec(raw)) !== null) {
    const body = m[1];
    // 行尾 `$` 锚定不可省：没有它，"portreaper" 会连 "portreaper-cli" 的块一起匹配，
    // 于是两个包都被当成第一个包处理（改对一个、漏掉另一个，且不报错）。
    if (new RegExp(`^name\\s*=\\s*"${crateName}"\\s*$`, "m").test(body)) {
      return { start: m.index, end: m.index + m[0].length, text: m[0] };
    }
  }
  return null;
}

export function setCargoLockVersion(raw, version, crateName = LOCK_CRATE_NAMES[0]) {
  const block = findCargoLockBlock(raw, crateName);
  if (!block) {
    invalid(`could not find [[package]] name = "${crateName}" block in Cargo.lock`);
  }
  const updated = block.text.replace(/^(version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
  // no-op ≠ 行缺失：版本已是目标值时 replace 结果与原文相同（幂等重跑合法），
  // 只有 version 行整个缺失才响亮失败 —— 与 setCargoTomlVersion 同一判据。
  // 旧写法把两者混为一谈，中断后的同版本重跑会在这里炸出误导性错误。
  if (updated === block.text && !/^version\s*=/m.test(block.text)) {
    invalid(`could not find version line for "${crateName}" in Cargo.lock`);
  }
  return raw.slice(0, block.start) + updated + raw.slice(block.end);
}

/** 一次性同步 Cargo.lock 里所有发布产物的版本。 */
export function setAllCargoLockVersions(raw, version) {
  let out = raw;
  for (const name of LOCK_CRATE_NAMES) {
    out = setCargoLockVersion(out, version, name);
  }
  return out;
}

// ---------------------------------------------------------------------------

async function collect() {
  const [pkg, conf] = await Promise.all([readJsonVersion(PKG_JSON), readJsonVersion(TAURI_CONF)]);
  const tomlRaw = await readFile(CARGO_TOML, "utf8");
  const cliTomlRaw = await readFile(CLI_CARGO_TOML, "utf8");
  const lockRaw = await readFile(CARGO_LOCK, "utf8");
  return [
    { name: "package.json", version: pkg },
    { name: "src-tauri/tauri.conf.json", version: conf },
    { name: "src-tauri/Cargo.toml", version: findCargoTomlVersion(tomlRaw) },
    { name: "crates/portreaper-cli/Cargo.toml", version: findCargoTomlVersion(cliTomlRaw) },
    ...LOCK_CRATE_NAMES.map((n) => ({
      name: `Cargo.lock [${n}]`,
      version: findCargoLockVersion(lockRaw, n),
    })),
  ];
}

async function runCheck(version) {
  const entries = await collect();
  const bad = entries.filter((e) => e.version !== version);
  if (bad.length === 0) {
    console.log(`OK: all files are at version ${version}.`);
    return;
  }
  console.error(`Version mismatch — expected ${version}:`);
  for (const e of entries) {
    const mark = e.version === version ? "OK " : "BAD";
    console.error(`  [${mark}] ${e.name}: ${e.version ?? "<not found>"}`);
  }
  process.exit(1);
}

async function runBump(version) {
  await writeJsonVersion(PKG_JSON, version);
  await writeJsonVersion(TAURI_CONF, version);

  const tomlRaw = await readFile(CARGO_TOML, "utf8");
  await writeFile(CARGO_TOML, setCargoTomlVersion(tomlRaw, version));

  const cliTomlRaw = await readFile(CLI_CARGO_TOML, "utf8");
  await writeFile(CLI_CARGO_TOML, setCargoTomlVersion(cliTomlRaw, version));

  const lockRaw = await readFile(CARGO_LOCK, "utf8");
  await writeFile(CARGO_LOCK, setAllCargoLockVersions(lockRaw, version));

  console.log(`Bumped version to ${version} in:`);
  console.log("  package.json");
  console.log("  src-tauri/tauri.conf.json");
  console.log("  src-tauri/Cargo.toml");
  console.log("  crates/portreaper-cli/Cargo.toml");
  console.log(`  Cargo.lock (${LOCK_CRATE_NAMES.join(", ")})`);
}

async function main() {
  const { check, version } = parseArgs(process.argv);
  if (check) await runCheck(version);
  else await runBump(version);
}

// 仅在作为脚本直接运行时执行（被 *.test.mjs import 时不触发，便于单元测试）。
// realpath 双侧归一（与三个 parity 守卫同一 idiom）：node 对主模块做 realpath 而
// argv[1] 保持原样，经 symlink 调用时裸比较不相等 → --check 门禁静默 exit 0。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  main().catch((err) => {
    fail(err?.stack || String(err));
  });
}
