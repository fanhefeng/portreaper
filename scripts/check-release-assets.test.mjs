// check-release-assets.mjs 的自测：真实源码必须通过，定向突变必须被拦截
//（守卫静默放行比没有守卫更危险 —— 与 check-reason-parity 同一纪律）。
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { checkAssetNames, checkWebsiteI18n } from "./check-release-assets.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const real = {
  releaseSrc: readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
  websiteSrc: readFileSync(join(root, "website/index.html"), "utf8"),
  readmeSrc: readFileSync(join(root, "README.md"), "utf8"),
  i18nSrc: readFileSync(join(root, "website/i18n.js"), "utf8"),
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
  const i18nSrc = real.i18nSrc.replace(
    /zh: \{/,
    'zh: {\n    "only.in.zh": "孤键",',
  );
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
