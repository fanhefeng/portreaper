#!/usr/bin/env node
// pnpm-workspace.yaml 里 vite-plus 版本钉版的一致性校验。
//
// 为什么需要：那个文件自己写着「升级 vite-plus 时下方 catalog 两条 +
// minimumReleaseAgeExclude 全部钉版条目必须一起改（共 12 处）——**漏改的 exclude
// 条目会静默失效**」。既然失效是静默的，就该有东西查它，否则那句警告只能靠人每次
// 都读到（评审发现：全仓库唯一一处自认「漏改会静默失效」却无守卫的地方）。
//
// 漏改一条 exclude 的后果：pnpm 的 minimumReleaseAge 会把那个平台包当成「太新」
// 而拒装，报错发生在**别人的机器**上（不同 CPU 架构的平台包各不相同），本机全绿。
//
// 口径：注释行不参与（那句警告里也带着一个版本号，算进去只会自己绊自己）。
//
// 用法：node scripts/check-workspace-versions.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/** catalog 的两条 + minimumReleaseAgeExclude 的十条，形如 `pkg@1.2.3` 或裸 `1.2.3`。 */
const VERSION_RE = /(\d+\.\d+\.\d+)/;

/**
 * 收集所有钉版 token（跳过注释行）。
 * 返回 `[{ line, version }]` —— 带行文本便于报错时指出是哪一条。
 */
export function collectPinnedVersions(yamlSrc) {
  const out = [];
  for (const raw of yamlSrc.split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    // 只看真正的钉版位置：catalog 的值、exclude 列表项
    if (!/^(-\s|vite:|vite-plus:)/.test(line)) continue;
    const m = line.match(VERSION_RE);
    if (m) out.push({ line, version: m[1] });
  }
  return out;
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkWorkspaceVersions(yamlSrc) {
  const pinned = collectPinnedVersions(yamlSrc);
  if (pinned.length === 0) {
    return [
      "pnpm-workspace.yaml 里一条钉版都没解析到 —— 文件形态变了时，" +
        "本守卫必须响亮失败而不是「没找到 = 没问题」",
    ];
  }
  const versions = [...new Set(pinned.map((p) => p.version))];
  if (versions.length > 1) {
    const detail = pinned.map((p) => `  ${p.version}  ← ${p.line}`).join("\n");
    return [
      `pnpm-workspace.yaml 的 vite-plus 钉版不一致（出现 ${versions.length} 个版本：` +
        `${versions.join(", ")}）—— 漏改的 exclude 条目会静默失效，` +
        `报错只会发生在别人的机器上：\n${detail}`,
    ];
  }
  return [];
}

// ---- CLI 入口（被 import 时不执行）----
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const src = readFileSync(join(root, "pnpm-workspace.yaml"), "utf8");
  const errors = checkWorkspaceVersions(src);
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\nworkspace version check FAILED (${errors.length}).`);
    process.exit(1);
  }
  const n = collectPinnedVersions(src).length;
  console.log(`✓ workspace versions OK — ${n} 处 vite-plus 钉版全部一致`);
}
