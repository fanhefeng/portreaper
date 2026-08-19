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

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ASSET_RE = /Portreaper-[A-Za-z0-9.-]+\.(?:dmg|exe)/g;

/** 稳定下载 URL 的路径前缀 —— 资产名对了但前缀有 typo 同样是 404（评审发现：
 *  旧校验只比对文件名集合，不看链接本身）。 */
const DOWNLOAD_PREFIX = "releases/latest/download/";

/**
 * 稳定名在 release.yml 里必须**真的被上传**，而不只是在 release notes 里被提到。
 *
 * 反例（评审实测）：把「Re-upload assets under stable names」整步删掉，只留 body.md
 * heredoc 里那三行 `` `Portreaper-macos-arm64.dmg` ``，全文扫的旧判据返回 0 错误。
 * 后果比单纯改名更狠 —— publish 的 `Verify expected assets exist` 检的全是**版本化**
 * 名（`^Portreaper_.*_aarch64\.dmg$`），所以 release 会一路绿灯发布，而 website 与
 * README 的全部下载链接同时 404，正是本守卫头注写的那个故障。
 *
 * 判据取 `dist/<name>`：`cp` 的目标与 `gh release upload` 的实参都是这个形态，
 * release notes 里的裸名不会带路径前缀。
 */
const UPLOAD_PREFIX = "dist/";

/** 真正把版本化产物改成稳定名再传上去的那一步。 */
const REUPLOAD_STEP = "Re-upload assets under stable names";

/** 从 workflow 里切出某个 step 的正文（到下一个同级 `- name:` 为止）。
 *  按 step 定界而不是全文扫：`echo "dist/Portreaper-…"` 这类无关文本也能让
 *  全文判据通过，而本守卫要断言的是「那一步确实还在、且确实处理了每个稳定名」。 */
function stepBody(src, stepName) {
  const at = src.indexOf(`- name: ${stepName}`);
  if (at === -1) return null;
  const next = src.indexOf("\n      - name: ", at + 1);
  return next === -1 ? src.slice(at) : src.slice(at, next);
}

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
    errors.push(
      `稳定资产名不一致：release.yml [${fmt(release)}] vs website/index.html [${fmt(website)}]`,
    );
  }
  if (!setEq(release, readme)) {
    errors.push(`稳定资产名不一致：release.yml [${fmt(release)}] vs README.md [${fmt(readme)}]`);
  }

  // 每个稳定名都必须真的被那一步上传/改名，而不只是在 notes 或注释里被提到
  const reupload = stepBody(releaseSrc, REUPLOAD_STEP);
  if (reupload === null) {
    errors.push(
      `release.yml 找不到「${REUPLOAD_STEP}」步骤 —— 稳定名全靠它产出；` +
        "步骤没了 publish 仍会绿灯（它只校验版本化名），而所有稳定下载链接会同时 404",
    );
  } else {
    for (const name of release) {
      if (!reupload.includes(UPLOAD_PREFIX + name)) {
        errors.push(
          `release.yml:「${REUPLOAD_STEP}」步骤里找不到 "${UPLOAD_PREFIX}${name}" ——` +
            " 这个稳定名只在别处的文本里出现过，没有任何一行真的产出它",
        );
      }
    }
  }

  // 每个出现的资产名必须至少带一条完整的稳定下载链接（链接文本里的裸名允许）
  for (const [label, src, names] of [
    ["website/index.html", websiteSrc, website],
    ["README.md", readmeSrc, readme],
  ]) {
    for (const name of names) {
      if (!src.includes(DOWNLOAD_PREFIX + name)) {
        errors.push(
          `${label}: "${name}" 缺少完整下载链接 "${DOWNLOAD_PREFIX}${name}"（URL 前缀损坏 → 下载 404）`,
        );
      }
    }
  }
  return errors;
}

/**
 * portreaper-cli 的稳定资产名：release.yml 的 `cli_asset` ↔ Raycast 扩展的
 * `CLI_ASSETS` 必须完全一致（纯函数，可测）。
 *
 * 与上面 dmg/exe 的校验是同一类问题、更高的代价：扩展**在用户机器上**按这些名字
 * 去 GitHub Release 下载引擎，改了一边就是 404 —— 而且不是「下载按钮坏了」这种
 * 一眼可见的故障，是「扩展装好了却起不来」。校验和文件名同理，少了它扩展就没法
 * 验完整性（Raycast Store 明确要求 hash verification）。
 */
export function checkCliAssetNames({ releaseSrc, installSrc }) {
  const errors = [];
  // `^\s+cli_asset:` 而非裸 `cli_asset:` —— 后者会把解释这个字段的**注释行**
  // （`# cli_asset: the STABLE name ...`）也当成一条配置，抓出个 "the" 来。
  // 自测里有一条就钉这个（守卫自身的 bug 同样会让门禁说谎）。
  const fromRelease = new Set(
    [...releaseSrc.matchAll(/^\s+cli_asset:\s*(\S+)/gm)].map((m) => m[1].trim()),
  );
  // 扩展侧：CLI_ASSETS 映射表的值 + 校验和文件常量
  const fromExt = new Set(
    [...installSrc.matchAll(/"(portreaper-cli-(?:macos|windows)[A-Za-z0-9.-]*)"/g)].map(
      (m) => m[1],
    ),
  );

  if (fromRelease.size === 0) {
    errors.push("release.yml 中未找到任何 cli_asset（matrix 字段被改名？提取正则失效？）");
  }
  if (fromExt.size === 0) {
    errors.push("Raycast 扩展的 install.ts 中未找到任何 portreaper-cli-* 资产名");
  }
  if (!setEq(fromRelease, fromExt)) {
    errors.push(
      `portreaper-cli 资产名不一致：release.yml [${fmt(fromRelease)}] vs ` +
        `integrations/raycast/src/install.ts [${fmt(fromExt)}] —— ` +
        `扩展会按这些名字去 GitHub Release 下载引擎，对不上即 404（扩展装好却起不来）`,
    );
  }

  // 校验和文件：release.yml 生成它，扩展消费它。两侧都必须提到同一个名字。
  const SUMS = "portreaper-cli-SHA256SUMS";
  if (!releaseSrc.includes(SUMS)) {
    errors.push(`release.yml 未产出 ${SUMS}（扩展无法校验下载完整性）`);
  }
  if (!installSrc.includes(SUMS)) {
    errors.push(`install.ts 未引用 ${SUMS}`);
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

/** website i18n 键齐全性（纯函数，可测）：返回错误信息数组（空 = 通过）。
 *  mainJsSrc 可选：main.js 里 t("…") 字面量引用的键同样必须存在 ——
 *  version.label / copy.done 只从 JS 消费，旧校验只扫 HTML 属性、改名不报错
 *  （评审发现的守卫盲区）。 */
export function checkWebsiteI18n({ i18nSrc, websiteSrc, mainJsSrc = "" }) {
  const errors = [];
  const dict = loadWebsiteDict(i18nSrc);
  const zhKeys = new Set(Object.keys(dict.zh));
  const enKeys = new Set(Object.keys(dict.en));

  for (const k of zhKeys) {
    if (!enKeys.has(k))
      errors.push(`website/i18n.js: en 字典缺键 "${k}"（英文界面会静默回退中文）`);
  }
  for (const k of enKeys) {
    if (!zhKeys.has(k)) errors.push(`website/i18n.js: zh 字典缺键 "${k}"`);
  }

  const used = [...websiteSrc.matchAll(/data-i18n(?:-aria)?="([^"]+)"/g)].map((m) => m[1]);
  for (const k of used) {
    if (!zhKeys.has(k)) {
      errors.push(`website/index.html 引用了字典中不存在的键 "${k}"（会渲染成裸键名）`);
    }
  }

  const usedJs = [...mainJsSrc.matchAll(/\bt\(\s*"([^"]+)"\s*\)/g)].map((m) => m[1]);
  for (const k of usedJs) {
    if (!zhKeys.has(k)) {
      errors.push(`website/main.js 引用了字典中不存在的键 "${k}"（会渲染成裸键名）`);
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一（评审发现）：node 对主模块做 realpath 而 argv[1] 保持原样，
// 经 symlink 调用时裸字符串比较不相等 → 脚本静默 exit 0、什么都没查 ——
// 这正是「守卫静默通过」这一最坏失效模式。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const websiteSrc = readFileSync(join(root, "website/index.html"), "utf8");
  const releaseSrc = readFileSync(join(root, ".github/workflows/release.yml"), "utf8");
  const errors = [
    ...checkAssetNames({
      releaseSrc,
      websiteSrc,
      readmeSrc: readFileSync(join(root, "README.md"), "utf8"),
    }),
    ...checkCliAssetNames({
      releaseSrc,
      installSrc: readFileSync(join(root, "integrations/raycast/src/install.ts"), "utf8"),
    }),
    ...checkWebsiteI18n({
      i18nSrc: readFileSync(join(root, "website/i18n.js"), "utf8"),
      websiteSrc,
      mainJsSrc: readFileSync(join(root, "website/main.js"), "utf8"),
    }),
  ];
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\n发布资产名 / website i18n parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log(
    "✓ release assets OK — 安装包与 CLI 的稳定资产名各方一致，website 字典键双语齐全且无悬空引用",
  );
}
