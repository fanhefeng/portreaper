#!/usr/bin/env node
// bundle identifier 的跨清单一致性校验。
//
// 为什么需要：目录解析有两份实现 —— portreaper-core 的 paths.rs（自解析，供
// CLI / Raycast 等无 Tauri 的前端用）与 Tauri 的 app_*_dir（GUI 用）。两者都以
// `<基目录>/<identifier>` 收尾，identifier 一旦分叉，GUI 与 CLI 就各写各的
// whitelist.json —— 用户在 Raycast 里加的星标，桌面版永远看不见。这个故障
// 不报错、不崩溃，只让人怀疑自己记错了，所以必须由守卫拦在提交前。
//
// 分工（两层都要，缺一不可）：
//   - 本脚本（静态）：只查 identifier 这个**常量**是否一致，CI/pre-push 都跑，
//     不需要启动应用；
//   - src-tauri/src/paths.rs 的 assert_matches_tauri（运行时）：查**算法**是否
//     一致（升级 tauri 后它换了 dirs 的 major、或改了某个目录的平台分叉）。
//     那个只有真实启动才能验证，debug 下 panic、release 下记 error。
//
// 用法：node scripts/check-paths-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/**
 * 提取 `pub const APP_IDENTIFIER: &str = "...";` 的值。
 *
 * 严格解析：找不到就抛，绝不返回 undefined 让调用方与同样 undefined 的另一侧
 * 「相等」而静默通过 —— 守卫静默放行比没有守卫更危险（同 check-reason-parity）。
 */
export function extractCoreIdentifier(pathsSrc) {
  const m = pathsSrc.match(/pub const APP_IDENTIFIER:\s*&str\s*=\s*"([^"]+)"\s*;/);
  if (!m) {
    throw new Error(
      '在 crates/portreaper-core/src/paths.rs 里找不到 `pub const APP_IDENTIFIER: &str = "...";` —— ' +
        "常量被改名或改形态时，本守卫必须响亮失败而不是放行",
    );
  }
  return m[1];
}

/** 提取 tauri.conf.json 顶层的 identifier。 */
export function extractTauriIdentifier(confSrc) {
  const conf = JSON.parse(confSrc);
  if (typeof conf.identifier !== "string" || conf.identifier === "") {
    throw new Error("src-tauri/tauri.conf.json 缺少顶层 identifier");
  }
  return conf.identifier;
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkPathsParity({ pathsSrc, tauriConfSrc }) {
  const errors = [];
  const core = extractCoreIdentifier(pathsSrc);
  const tauri = extractTauriIdentifier(tauriConfSrc);
  if (core !== tauri) {
    errors.push(
      `bundle identifier 不一致：portreaper_core::paths::APP_IDENTIFIER = "${core}"，` +
        `src-tauri/tauri.conf.json identifier = "${tauri}" —— ` +
        `GUI 与 CLI/Raycast 会各写各的 whitelist.json（星标互相看不见）`,
    );
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一：symlink 调用下裸比较不相等 → 守卫静默 exit 0，比没有守卫更糟。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = checkPathsParity({
    pathsSrc: readFileSync(join(root, "crates/portreaper-core/src/paths.rs"), "utf8"),
    tauriConfSrc: readFileSync(join(root, "src-tauri/tauri.conf.json"), "utf8"),
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\nbundle identifier parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log("✓ paths parity OK — core 与 tauri.conf.json 的 bundle identifier 一致");
}
