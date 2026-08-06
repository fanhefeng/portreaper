// check-release-assets.mjs 的自测：真实源码必须通过，定向突变必须被拦截
//（守卫静默放行比没有守卫更危险 —— 与 check-reason-parity 同一纪律）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkAssetNames, checkCliAssetNames, checkWebsiteI18n } from "./check-release-assets.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  releaseSrc: readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
  websiteSrc: readFileSync(join(root, "website/index.html"), "utf8"),
  readmeSrc: readFileSync(join(root, "README.md"), "utf8"),
  installSrc: readFileSync(join(root, "integrations/raycast/src/install.ts"), "utf8"),
  i18nSrc: readFileSync(join(root, "website/i18n.js"), "utf8"),
  mainJsSrc: readFileSync(join(root, "website/main.js"), "utf8"),
};

test("真实源码当前必须通过资产名一致性校验", () => {
  assert.deepEqual(checkAssetNames(real), []);
});

test("真实源码当前必须通过 website i18n 键校验", () => {
  assert.deepEqual(checkWebsiteI18n(real), []);
});

test("release.yml 改稳定名必须被拦截", () => {
  const releaseSrc = real.releaseSrc.replaceAll(
    "Portreaper-macos-arm64.dmg",
    "Portreaper-macos-aarch64.dmg",
  );
  const errors = checkAssetNames({ ...real, releaseSrc });
  assert.ok(errors.some((e) => e.includes("不一致")));
});

test("website 下载链接改名必须被拦截", () => {
  const websiteSrc = real.websiteSrc.replace(
    "Portreaper-windows-x64-setup.exe",
    "Portreaper-win64-setup.exe",
  );
  const errors = checkAssetNames({ ...real, websiteSrc });
  assert.ok(errors.some((e) => e.includes("不一致")));
});

test("README 漏掉一个资产必须被拦截", () => {
  const readmeSrc = real.readmeSrc.replaceAll("Portreaper-macos-x64.dmg", "");
  const errors = checkAssetNames({ ...real, readmeSrc });
  assert.ok(errors.some((e) => e.includes("README.md")));
});

test("en 字典缺键必须被拦截（website 无 tsc 兜底）", () => {
  // 在 zh 字典里加一个 en 没有的键
  const i18nSrc = real.i18nSrc.replace(/zh: \{/, 'zh: {\n    "only.in.zh": "孤键",');
  const errors = checkWebsiteI18n({ ...real, i18nSrc });
  assert.ok(errors.some((e) => e.includes('"only.in.zh"') && e.includes("en 字典缺键")));
});

test("index.html 引用不存在的 data-i18n 键必须被拦截", () => {
  const websiteSrc = real.websiteSrc.replace(
    'data-i18n="nav.features"',
    'data-i18n="nav.ghost_key"',
  );
  const errors = checkWebsiteI18n({ ...real, websiteSrc });
  assert.ok(errors.some((e) => e.includes('"nav.ghost_key"')));
});

test("下载链接 URL 前缀损坏必须被拦截（资产名集合仍一致的隐性 404）", () => {
  // 名字不变、只坏前缀：旧校验（纯文件名集合比对）对此完全放行
  const websiteSrc = real.websiteSrc.replaceAll(
    "releases/latest/download/Portreaper-macos-arm64.dmg",
    "releases/download/latest/Portreaper-macos-arm64.dmg",
  );
  const errors = checkAssetNames({ ...real, websiteSrc });
  assert.ok(errors.some((e) => e.includes("缺少完整下载链接")));
});

test("main.js 引用不存在的字典键必须被拦截（version.label / copy.done 只从 JS 消费）", () => {
  const mainJsSrc = real.mainJsSrc.replace('t("version.label")', 't("version.ghost_key")');
  const errors = checkWebsiteI18n({ ...real, mainJsSrc });
  assert.ok(errors.some((e) => e.includes('"version.ghost_key"') && e.includes("main.js")));
});

// ---- portreaper-cli 的稳定资产名（release.yml ↔ Raycast 扩展）----
// 这组名字是跨机器的契约：扩展在用户电脑上按它们去 GitHub Release 下载引擎，
// 对不上不是「下载按钮坏了」，而是「扩展装好却起不来」。

test("CLI 资产名当前必须一致", () => {
  assert.deepEqual(checkCliAssetNames(real), []);
});

test("release.yml 改了 cli_asset 而扩展没跟，必须被拦截", () => {
  const releaseSrc = real.releaseSrc.replace(
    "portreaper-cli-macos-arm64",
    "portreaper-cli-macos-aarch64",
  );
  const errors = checkCliAssetNames({ ...real, releaseSrc });
  assert.ok(
    errors.some((e) => /资产名不一致/.test(e)),
    `应报资产名不一致，实际: ${JSON.stringify(errors)}`,
  );
});

test("release.yml 不再产出 SHA256SUMS，必须被拦截", () => {
  const releaseSrc = real.releaseSrc.replaceAll("portreaper-cli-SHA256SUMS", "checksums.txt");
  const errors = checkCliAssetNames({ ...real, releaseSrc });
  assert.ok(
    errors.some((e) => /未产出 portreaper-cli-SHA256SUMS/.test(e)),
    `应报缺校验和文件，实际: ${JSON.stringify(errors)}`,
  );
});

// 守卫自身的 bug 同样会让门禁说谎：裸 `cli_asset:` 正则会把**解释该字段的注释行**
// 也当成一条配置（实际踩到过，抓出个 "the"）。这条钉住「注释不算数」。
test("解释 cli_asset 的注释行不得被当成一条资产名", () => {
  const releaseSrc = real.releaseSrc.replace(
    "        # cli_asset:",
    "        # cli_asset: bogus-name-from-a-comment\n        # cli_asset:",
  );
  // 突变依赖注释行的精确缩进：缩进一变，replace 变成 no-op，这条用例就在
  // 「校验原始源码」而不是校验注释豁免，且会一直绿着（评审发现）
  assert.notEqual(
    releaseSrc,
    real.releaseSrc,
    "突变未生效：release.yml 的注释缩进已变，用例形同虚设",
  );
  assert.deepEqual(checkCliAssetNames({ ...real, releaseSrc }), []);
});
