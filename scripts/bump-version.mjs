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
//   src-tauri/Cargo.lock          [[package]] name="portreaper" -> version
//
// Zero dependencies. Requires Node >= 18 (ESM, fs/promises).

import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { dirname, join } from 'node:path';

const __dirname = dirname(fileURLToPath(import.meta.url));
const ROOT = join(__dirname, '..');

const PKG_JSON = join(ROOT, 'package.json');
const TAURI_CONF = join(ROOT, 'src-tauri', 'tauri.conf.json');
const CARGO_TOML = join(ROOT, 'src-tauri', 'Cargo.toml');
const CARGO_LOCK = join(ROOT, 'src-tauri', 'Cargo.lock');

// semver-ish: major.minor.patch with an optional pre-release/build suffix.
// 每个数字段禁前导零（0 | [1-9]\d*），与 cargo 一致 —— 否则 01.2.3 能写进 JSON
// 清单，却被 cargo 拒绝，造成跨清单半应用的 bump（评审 D1）。
export const SEMVER_RE =
  /^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

const CRATE_NAME = 'portreaper';

function fail(msg) {
  console.error(`error: ${msg}`);
  process.exit(1);
}

function parseArgs(argv) {
  const args = argv.slice(2);
  let check = false;
  const positionals = [];
  for (const a of args) {
    if (a === '--check') check = true;
    else if (a === '--help' || a === '-h') {
      console.log(
        'Usage:\n' +
          '  node scripts/bump-version.mjs <version>\n' +
          '  node scripts/bump-version.mjs --check <version>',
      );
      process.exit(0);
    } else positionals.push(a);
  }
  const version = positionals[0];
  if (version === undefined || version === '') {
    fail('missing version argument (e.g. `node scripts/bump-version.mjs 0.2.0`)');
  }
  if (!SEMVER_RE.test(version)) {
    fail(`"${version}" is not a valid version (expected MAJOR.MINOR.PATCH[-suffix])`);
  }
  return { check, version };
}

// --- JSON helpers: preserve 2-space indent + single trailing newline. -------

async function readJsonVersion(path) {
  const raw = await readFile(path, 'utf8');
  const data = JSON.parse(raw);
  return data.version;
}

async function writeJsonVersion(path, version) {
  const raw = await readFile(path, 'utf8');
  const data = JSON.parse(raw);
  data.version = version;
  await writeFile(path, JSON.stringify(data, null, 2) + '\n');
}

// --- Cargo.toml: rewrite the version in the [package] section only. ---------
// We scope the search to the [package] table so a `version = "..."` in some
// other table (e.g. a dependency) is never matched.

export function findCargoTomlVersion(raw) {
  const pkg = sliceTomlSection(raw, 'package');
  const m = pkg.match(/^\s*version\s*=\s*"([^"]*)"/m);
  return m ? m[1] : undefined;
}

function sliceTomlSection(raw, section) {
  const header = new RegExp(`^\\[${section}\\]\\s*$`, 'm');
  const start = raw.search(header);
  if (start === -1) return '';
  const after = raw.slice(start + raw.match(header)[0].length);
  // Stop at the next top-level table header `[...]` (not an array `[[...]]`
  // continuation — a fresh `[` at line start ends the section either way).
  const next = after.search(/^\[/m);
  return next === -1 ? after : after.slice(0, next);
}

export function setCargoTomlVersion(raw, version) {
  const header = /^\[package\]\s*$/m;
  const start = raw.search(header);
  if (start === -1) fail('could not find [package] section in Cargo.toml');
  const headerLen = raw.match(header)[0].length;
  const sectionStart = start + headerLen;
  const after = raw.slice(sectionStart);
  const nextTable = after.search(/^\[/m);
  const sectionEnd = nextTable === -1 ? raw.length : sectionStart + nextTable;

  const section = raw.slice(sectionStart, sectionEnd);
  const replaced = section.replace(
    /^(\s*version\s*=\s*")[^"]*(")/m,
    `$1${version}$2`,
  );
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
  const re = /\[\[package\]\]\n([\s\S]*?)(?=\n\[\[package\]\]|\n*$)/g;
  let m;
  while ((m = re.exec(raw)) !== null) {
    const body = m[1];
    if (new RegExp(`^name\\s*=\\s*"${CRATE_NAME}"\\s*$`, 'm').test(body)) {
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
  const updated = block.text.replace(
    /^(version\s*=\s*")[^"]*(")/m,
    `$1${version}$2`,
  );
  if (updated === block.text) {
    fail(`could not find version line for "${CRATE_NAME}" in Cargo.lock`);
  }
  return raw.slice(0, block.start) + updated + raw.slice(block.end);
}

// ---------------------------------------------------------------------------

async function collect() {
  const [pkg, conf] = await Promise.all([
    readJsonVersion(PKG_JSON),
    readJsonVersion(TAURI_CONF),
  ]);
  const tomlRaw = await readFile(CARGO_TOML, 'utf8');
  const lockRaw = await readFile(CARGO_LOCK, 'utf8');
  return [
    { name: 'package.json', version: pkg },
    { name: 'src-tauri/tauri.conf.json', version: conf },
    { name: 'src-tauri/Cargo.toml', version: findCargoTomlVersion(tomlRaw) },
    { name: 'src-tauri/Cargo.lock', version: findCargoLockVersion(lockRaw) },
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
    const mark = e.version === version ? 'OK ' : 'BAD';
    console.error(`  [${mark}] ${e.name}: ${e.version ?? '<not found>'}`);
  }
  process.exit(1);
}

async function runBump(version) {
  await writeJsonVersion(PKG_JSON, version);
  await writeJsonVersion(TAURI_CONF, version);

  const tomlRaw = await readFile(CARGO_TOML, 'utf8');
  await writeFile(CARGO_TOML, setCargoTomlVersion(tomlRaw, version));

  const lockRaw = await readFile(CARGO_LOCK, 'utf8');
  await writeFile(CARGO_LOCK, setCargoLockVersion(lockRaw, version));

  console.log(`Bumped version to ${version} in:`);
  console.log('  package.json');
  console.log('  src-tauri/tauri.conf.json');
  console.log('  src-tauri/Cargo.toml');
  console.log('  src-tauri/Cargo.lock');
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
