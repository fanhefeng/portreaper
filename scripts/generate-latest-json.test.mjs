// generate-latest-json.mjs 的自测（node --test，随 CI / pre-push 的 glob 跑）。
// 守卫的失效模式是「静默生成残缺源」：平台齐全性与命名映射必须钉住。
import { test } from "node:test";
import assert from "node:assert/strict";

import { generateLatestJson } from "./generate-latest-json.mjs";

const SIG = "dGVzdC1zaWduYXR1cmU=\n";

function fullAssets() {
  return [
    { name: "Portreaper_0.11.0_aarch64.app.tar.gz", sig: SIG },
    { name: "Portreaper_0.11.0_x64.app.tar.gz", sig: SIG },
    { name: "Portreaper_0.11.0_x64-setup.exe", sig: SIG },
  ];
}

const BASE = {
  tag: "v0.11.0",
  repo: "fanhefeng/portreaper",
  pubDate: "2026-08-18T00:00:00.000Z",
};

test("三平台齐全时生成完整 latest.json（键名 / URL / 版本去 v 前缀）", () => {
  const json = generateLatestJson({ ...BASE, assets: fullAssets(), notes: "n" });
  assert.equal(json.version, "0.11.0");
  assert.equal(json.notes, "n");
  assert.equal(json.pub_date, BASE.pubDate);
  assert.deepEqual(Object.keys(json.platforms).sort(), [
    "darwin-aarch64",
    "darwin-x86_64",
    "windows-x86_64",
  ]);
  assert.equal(
    json.platforms["darwin-aarch64"].url,
    "https://github.com/fanhefeng/portreaper/releases/download/v0.11.0/Portreaper_0.11.0_aarch64.app.tar.gz",
  );
  // 签名原样透传（仅去首尾空白）—— updater 直接拿它做 minisign 校验
  assert.equal(json.platforms["windows-x86_64"].signature, "dGVzdC1zaWduYXR1cmU=");
});

test("aarch64 包不会被 x64 模式误吃（_x64 与 _aarch64 的后缀区分）", () => {
  const json = generateLatestJson({ ...BASE, assets: fullAssets() });
  assert.match(json.platforms["darwin-x86_64"].url, /_x64\.app\.tar\.gz$/);
  assert.match(json.platforms["darwin-aarch64"].url, /_aarch64\.app\.tar\.gz$/);
});

test("缺任一平台 → 响亮失败，绝不生成残缺源", () => {
  const assets = fullAssets().slice(0, 2); // 丢掉 Windows
  assert.throws(() => generateLatestJson({ ...BASE, assets }), /windows-x86_64/);
});

test("混入认不出的 .sig（如未来的 msi）→ 失败而非静默忽略", () => {
  const assets = [...fullAssets(), { name: "Portreaper_0.11.0_x64_en-US.msi", sig: SIG }];
  assert.throws(() => generateLatestJson({ ...BASE, assets }), /命中 0 个平台模式/);
});

test("同平台出现两个资产（上传串台）→ 失败", () => {
  const assets = [...fullAssets(), { name: "Portreaper_0.11.1_x64-setup.exe", sig: SIG }];
  assert.throws(() => generateLatestJson({ ...BASE, assets }), /出现了两个资产/);
});

test("空签名内容 → 失败（一份没法校验的 latest.json 等于给所有用户断更）", () => {
  const assets = fullAssets();
  assets[0].sig = "   \n";
  assert.throws(() => generateLatestJson({ ...BASE, assets }), /签名内容为空/);
});

test("tag / repo / 资产名形态校验", () => {
  assert.throws(() => generateLatestJson({ ...BASE, tag: "0.11.0", assets: fullAssets() }));
  assert.throws(() => generateLatestJson({ ...BASE, repo: "not-a-repo", assets: fullAssets() }));
  const assets = fullAssets();
  assets[0].name = "Portreaper 0.11.0_aarch64.app.tar.gz"; // 空格进 URL
  assert.throws(() => generateLatestJson({ ...BASE, assets }), /意外字符/);
});
