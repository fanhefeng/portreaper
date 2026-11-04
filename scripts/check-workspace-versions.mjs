#!/usr/bin/env node
// pnpm-workspace.yaml 里 vite-plus 版本钉版的一致性校验。
//
// catalog.vite 必须指向 vite-plus-core，且与 catalog.vite-plus 使用相同的精确版本。
// minimumReleaseAgeExclude 是可选配置：Vite+ 1.x 升级已移除旧版发布等待例外，
// 不再要求固定的条目数量。若以后恢复 Vite+ 例外，其版本仍必须与 catalog 同步；
// 漏改平台包的 exclude 会静默失效，拒装只在别人的 CPU 架构上出现。
// 仅检查这两个段落中的 Vite+ 条目，忽略注释、其他依赖及 overrides。
//
// 用法：node scripts/check-workspace-versions.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const VERSION = "(\\d+\\.\\d+\\.\\d+(?:-[0-9A-Za-z.-]+)?(?:\\+[0-9A-Za-z.-]+)?)";
const CORE_PIN_RE = new RegExp(`^npm:@voidzero-dev/vite-plus-core@${VERSION}$`);
const CLI_PIN_RE = new RegExp(`^${VERSION}$`);
const EXCLUDE_PIN_RE = new RegExp(`^(?:vite-plus|@voidzero-dev/vite-plus-[a-z0-9-]+)@${VERSION}$`);

// 只支持本文件所用的 YAML 标量形式；复杂写法须显式适配，不能漏掉必需项后放行。
function scalar(src) {
  const value = src.replace(/\s+#.*$/, "").trim();
  if (value.startsWith('"') || value.startsWith("'")) {
    return value.length > 1 && value.at(-1) === value[0] ? value.slice(1, -1) : "";
  }
  return value;
}

function readWorkspacePins(yamlSrc) {
  const pinned = [];
  const errors = [];
  const catalogCounts = new Map([
    ["vite", 0],
    ["vite-plus", 0],
  ]);
  let section;

  for (const raw of yamlSrc.split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    if (!/^\s/.test(raw)) {
      section = line.match(/^(catalog|minimumReleaseAgeExclude):\s*(?:#.*)?$/)?.[1];
      continue;
    }

    let version;
    if (section === "catalog") {
      const entry = line.match(/^(vite|vite-plus):\s*(.*)$/);
      if (!entry) continue;
      const [, name, src] = entry;
      catalogCounts.set(name, catalogCounts.get(name) + 1);
      version = scalar(src).match(name === "vite" ? CORE_PIN_RE : CLI_PIN_RE)?.[1];
      if (!version) errors.push(`catalog.${name} 必须使用正确的 Vite+ 精确钉版：${line}`);
    } else if (section === "minimumReleaseAgeExclude") {
      const entry = line.match(/^-\s+(.+)$/);
      if (!entry) continue;
      const value = scalar(entry[1]);
      if (!/^(?:vite-plus(?:@|$)|@voidzero-dev\/vite-plus-)/.test(value)) continue;
      version = value.match(EXCLUDE_PIN_RE)?.[1];
      if (!version) errors.push(`minimumReleaseAgeExclude 的 Vite+ 条目必须精确钉版：${line}`);
    }
    if (version) pinned.push({ line, version });
  }

  for (const [name, count] of catalogCounts) {
    if (count !== 1) errors.push(`catalog.${name} 必须恰好声明一次（实际 ${count} 次）`);
  }
  return { pinned, errors };
}

/**
 * 收集 catalog 与可选发布等待例外中的 Vite+ 精确钉版（跳过注释和其他依赖）。
 * 返回 `[{ line, version }]` —— 带行文本便于报错时指出是哪一条。
 */
export function collectPinnedVersions(yamlSrc) {
  return readWorkspacePins(yamlSrc).pinned;
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkWorkspaceVersions(yamlSrc) {
  const { pinned, errors } = readWorkspacePins(yamlSrc);
  if (pinned.length === 0) {
    return [
      "pnpm-workspace.yaml 里一条钉版都没解析到 —— 文件形态变了时，" +
        "本守卫必须响亮失败而不是「没找到 = 没问题」",
    ];
  }
  if (errors.length > 0) return errors;
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
