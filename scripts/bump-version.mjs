#!/usr/bin/env node
// bump-version.mjs — keep the app version in lockstep across all manifests.
//
// Usage:
//   node scripts/bump-version.mjs 0.2.0          rewrite the version everywhere
//   node scripts/bump-version.mjs --check 0.2.0  verify everything already agrees
//
// Touches four files (paths are relative to the repo root):
//   package.json                 .version          (JSON)
//   src-tauri/tauri.conf.json     .version          (JSON)
//   src-tauri/Cargo.toml          [package] version (TOML, first match only)
//   Cargo.lock                    [[package]] name="portreaper" -> version
//
// Cargo.lock 住在仓库根（workspace 根也在那里），而 Cargo.toml 仍在 src-tauri/ ——
// 这不是笔误：src-tauri 只是 workspace 的一个成员 crate，lockfile 属于整个
// workspace。同理，crates/portreaper-core 的版本**不**归本脚本管（它是不发布的
// 内部 crate，版本与应用版本解耦）。
//
// Zero dependencies. Requires Node >= 18 (ESM, fs/promises).

import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, "..");

const PKG_JSON = join(ROOT, "package.json");
const TAURI_CONF = join(ROOT, "src-tauri", "tauri.conf.json");
const CARGO_TOML = join(ROOT, "src-tauri", "Cargo.toml");
const CARGO_LOCK = join(ROOT, "Cargo.lock"); // workspace 根，非 src-tauri/

// semver-ish: major.minor.patch with an optional pre-release/build suffix.
// 每个数字段禁前导零（0 | [1-9]\d*），与 cargo 一致 —— 否则 01.2.3 能写进 JSON
// 清单，却被 cargo 拒绝，造成跨清单半应用的 bump（评审 D1）。
export const SEMVER_RE =
  /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const CRATE_NAME = "portreaper";

function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
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
  if (start === -1) fail("could not find [package] section in Cargo.toml");
  const headerLen = raw.match(header)[0].length;
  const sectionStart = start + headerLen;
  const after = raw.slice(sectionStart);
  const nextTable = after.search(/^\[/m);
  const sectionEnd = nextTable === -1 ? raw.length : sectionStart + nextTable;

  const section = raw.slice(sectionStart, sectionEnd);
  const replaced = section.replace(/^(\s*version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
  if (replaced === section && !/^\s*version\s*=/m.test(section)) {
    fail('could not find `version = "..."` in [package] section of Cargo.toml');
  }
  return raw.slice(0, sectionStart) + replaced + raw.slice(sectionEnd);
}

// --- Cargo.lock: rewrite the version of the `portreaper` [[package]] block. -

export function findCargoLockVersion(raw) {
  const block = findCargoLockBlock(raw);
  if (!block) return undefined;
  const m = block.text.match(/^version\s*=\s*"([^"]*)"/m);
  return m ? m[1] : undefined;
}

function findCargoLockBlock(raw) {
  // Each entry is `[[package]]\nname = "..."\nversion = "..."\n...`.
  // \r?\n：.gitattributes 已钉 LF，但 Windows 上 autocrlf=true 的旧检出
  // 仍可能是 CRLF —— 容忍它，避免脚本在那种环境响亮失败（评审发现）。
  const re = /\[\[package\]\]\r?\n([\s\S]*?)(?=\r?\n\[\[package\]\]|(?:\r?\n)*$)/g;
  let m;
  while ((m = re.exec(raw)) !== null) {
    const body = m[1];
    if (new RegExp(`^name\\s*=\\s*"${CRATE_NAME}"\\s*$`, "m").test(body)) {
      return { start: m.index, end: m.index + m[0].length, text: m[0] };
    }
  }
  return null;
}

export function setCargoLockVersion(raw, version) {
  const block = findCargoLockBlock(raw);
  if (!block) {
    fail(`could not find [[package]] name = "${CRATE_NAME}" block in Cargo.lock`);
  }
  const updated = block.text.replace(/^(version\s*=\s*")[^"]*(")/m, `$1${version}$2`);
  if (updated === block.text) {
    fail(`could not find version line for "${CRATE_NAME}" in Cargo.lock`);
  }
  return raw.slice(0, block.start) + updated + raw.slice(block.end);
}

// ---------------------------------------------------------------------------

async function collect() {
  const [pkg, conf] = await Promise.all([readJsonVersion(PKG_JSON), readJsonVersion(TAURI_CONF)]);
  const tomlRaw = await readFile(CARGO_TOML, "utf8");
  const lockRaw = await readFile(CARGO_LOCK, "utf8");
  return [
    { name: "package.json", version: pkg },
    { name: "src-tauri/tauri.conf.json", version: conf },
    { name: "src-tauri/Cargo.toml", version: findCargoTomlVersion(tomlRaw) },
    { name: "Cargo.lock", version: findCargoLockVersion(lockRaw) },
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

  const lockRaw = await readFile(CARGO_LOCK, "utf8");
  await writeFile(CARGO_LOCK, setCargoLockVersion(lockRaw, version));

  console.log(`Bumped version to ${version} in:`);
  console.log("  package.json");
  console.log("  src-tauri/tauri.conf.json");
  console.log("  src-tauri/Cargo.toml");
  console.log("  Cargo.lock");
}

async function main() {
  const { check, version } = parseArgs(process.argv);
  if (check) await runCheck(version);
  else await runBump(version);
}

// 仅在作为脚本直接运行时执行（被 *.test.mjs import 时不触发，便于单元测试）。
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((err) => {
    fail(err?.stack || String(err));
  });
}
