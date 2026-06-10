#!/usr/bin/env node
// 发布资产稳定名 ↔ 下载链接的机械一致性校验（评审发现：三处硬编码互相无守卫，
// 任何一处改名都不会被拦截 —— 下载按钮 404 只能等用户发现）：
//   1. release.yml（publish 步骤生成的稳定名）、website/index.html（下载链接）、
//      README.md（下载表格）三处提取出的稳定资产名集合必须完全一致；
//   2. website/i18n.js 的 zh / en 字典键集合必须相等（主应用有 tsc 兜底，
//      website 没有任何构建步骤 —— en 缺键会静默回退中文）；
//   3. website/index.html 引用的每个 data-i18n / data-i18n-aria 键必须存在于字典。
//
// 用法：node scripts/check-release-assets.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs           （check-release-assets.test.mjs）

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ASSET_RE = /Portreaper-[A-Za-z0-9.-]+\.(?:dmg|exe)/g;

function assetSet(source) {
  return new Set(source.match(ASSET_RE) ?? []);
}

function setEq(a, b) {
  return a.size === b.size && [...a].every((x) => b.has(x));
}

const fmt = (s) => [...s].sort().join(", ") || "(空)";

/** 稳定资产名三处一致性（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkAssetNames({ releaseSrc, websiteSrc, readmeSrc }) {
  const errors = [];
  const release = assetSet(releaseSrc);
  const website = assetSet(websiteSrc);
  const readme = assetSet(readmeSrc);

  if (release.size === 0) errors.push("release.yml 中未找到任何稳定资产名（提取正则失效？）");
  if (!setEq(release, website)) {
    errors.push(`稳定资产名不一致：release.yml [${fmt(release)}] vs website/index.html [${fmt(website)}]`);
  }
  if (!setEq(release, readme)) {
    errors.push(`稳定资产名不一致：release.yml [${fmt(release)}] vs README.md [${fmt(readme)}]`);
  }
  return errors;
}

/** website 字典：执行 i18n.js（自家代码）拿到 window.I18N。 */
function loadWebsiteDict(i18nSrc) {
  const sandbox = {};
  new Function("window", i18nSrc)(sandbox);
  if (!sandbox.I18N || !sandbox.I18N.zh || !sandbox.I18N.en) {
    throw new Error("website/i18n.js 未产出 window.I18N.{zh,en}");
  }
  return sandbox.I18N;
}

/** website i18n 键齐全性（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkWebsiteI18n({ i18nSrc, websiteSrc }) {
  const errors = [];
  const dict = loadWebsiteDict(i18nSrc);
  const zhKeys = new Set(Object.keys(dict.zh));
  const enKeys = new Set(Object.keys(dict.en));

  for (const k of zhKeys) {
    if (!enKeys.has(k)) errors.push(`website/i18n.js: en 字典缺键 "${k}"（英文界面会静默回退中文）`);
  }
  for (const k of enKeys) {
    if (!zhKeys.has(k)) errors.push(`website/i18n.js: zh 字典缺键 "${k}"`);
  }

  const used = [
    ...websiteSrc.matchAll(/data-i18n(?:-aria)?="([^"]+)"/g),
  ].map((m) => m[1]);
  for (const k of used) {
    if (!zhKeys.has(k)) {
      errors.push(`website/index.html 引用了字典中不存在的键 "${k}"（会渲染成裸键名）`);
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const websiteSrc = readFileSync(join(root, "website/index.html"), "utf8");
  const errors = [
    ...checkAssetNames({
      releaseSrc: readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
      websiteSrc,
      readmeSrc: readFileSync(join(root, "README.md"), "utf8"),
    }),
    ...checkWebsiteI18n({
      i18nSrc: readFileSync(join(root, "website/i18n.js"), "utf8"),
      websiteSrc,
    }),
  ];
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\n发布资产名 / website i18n parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log("✓ release assets OK — 稳定资产名三处一致，website 字典键双语齐全且无悬空引用");
}
