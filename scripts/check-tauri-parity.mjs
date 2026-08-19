#!/usr/bin/env node
// Tauri 生态「Rust crate ↔ npm 包」的 major.minor 一致性校验。
//
// 为什么需要：tauri 的每个部件都是**一对**包 —— Rust crate 与同名 npm 包
// （tauri/@tauri-apps/api、tauri-plugin-log/@tauri-apps/plugin-log、…），
// 且 `tauri build` 会在构建**开始前**拒绝 major.minor 不一致的组合：
//
//     Error Found version mismatched Tauri packages. Make sure the NPM
//     package and Rust crate versions are on the same major/minor releases
//
// 这条错误只在 `tauri build` / `tauri dev` 里出现。本项目的 CI「Check」腿与
// pre-push 钩子**都不跑 tauri build**（太慢），于是这类不一致可以一路全绿地
// 合进 main，直到 push tag 之后、三条 release 构建腿同时炸掉才被发现 ——
// 那时 draft release 已经建好，只能删 tag 重来。v0.9.0 就是这么炸的一次：
// dependabot 把 Rust 侧 tauri-plugin-log 抬到 2.9.0，npm 侧还停在 2.8.0。
//
// **dependabot 结构性地看不见这个约束**：Rust 与 npm 是两个独立生态，它对
// cargo 的更新永远不会连带动 package.json。所以这不是「小心一点就能避免」的
// 事，必须由守卫兜住。
//
// 口径：只比 major.minor（patch 允许漂移，tauri 自己的检查也只看到 minor）。
// 配对表**从 package.json 推导**，不再手工维护（评审发现：手工表的头注自己承认
// 「漏加不会被自动发现」，而这个守卫存在的唯一理由就是没有别的东西能发现它 ——
// 一张会漏的表守着一个只有它能守的不变量，等于没守）。
//
// 用法：node scripts/check-tauri-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/**
 * 不参与配对的 npm 包 —— 构建工具，没有对应的 Rust crate，tauri 自己的检查也不管。
 * （`tauri-build` 同理，但它只在 Rust 侧，本来就进不了这份从 npm 推导的表。）
 */
const NPM_ONLY = new Set(["@tauri-apps/cli"]);

/**
 * 从 `package.json` 推导 Rust crate ↔ npm 包的配对表。
 *
 * 映射规则就两条：`@tauri-apps/api` → `tauri`，`@tauri-apps/plugin-X` →
 * `tauri-plugin-X`。**纯 Rust 侧的插件天然不在表内**（updater / window-state /
 * single-instance 在 npm 侧根本没有条目），这正是想要的 —— 它们没有可漂移的两侧。
 *
 * 推导取代手工表的理由：手工表漏加一对不会被任何东西发现，而这个守卫守的偏偏是
 * 「除了 `tauri build` 没人看得见」的那类不一致（评审发现）。今天加一对
 * `@tauri-apps/plugin-dialog` + `tauri-plugin-dialog` 并把版本错开，旧表照样绿。
 */
export function derivePairs(pkgJsonSrc) {
  const pkg = JSON.parse(pkgJsonSrc);
  const names = new Set([
    ...Object.keys(pkg.dependencies ?? {}),
    ...Object.keys(pkg.devDependencies ?? {}),
  ]);
  const pairs = [];
  for (const npm of [...names].sort()) {
    if (!npm.startsWith("@tauri-apps/") || NPM_ONLY.has(npm)) continue;
    if (npm === "@tauri-apps/api") {
      pairs.push({ crate: "tauri", npm });
      continue;
    }
    const plugin = npm.match(/^@tauri-apps\/plugin-(.+)$/);
    if (plugin) {
      pairs.push({ crate: `tauri-plugin-${plugin[1]}`, npm });
      continue;
    }
    throw new Error(
      `认不出的 @tauri-apps 包 "${npm}" —— 它要么该进 NPM_ONLY（构建工具），` +
        "要么需要在这里补一条映射规则；守卫拒绝静默跳过",
    );
  }
  if (pairs.length === 0) {
    throw new Error(
      "从 package.json 推导不出任何 tauri 配对 —— 依赖被挪走或字段改名时，" +
        "本守卫必须响亮失败而不是「没找到 = 没问题」",
    );
  }
  return pairs;
}

/** "2.11.5" → "2.11"。取不出 major.minor 就抛，绝不返回 undefined 让两侧同为
 *  undefined 地「相等」通过（同 check-toolchain-parity 的取舍）。 */
export function majorMinor(version, label) {
  const m = String(version).match(/^\D*(\d+)\.(\d+)/);
  if (!m) {
    throw new Error(`${label} 的版本号 "${version}" 解析不出 major.minor`);
  }
  return `${m[1]}.${m[2]}`;
}

/**
 * 从 Cargo.lock 里取某个 crate 的版本。
 *
 * 按 `[[package]]` 块解析而不是全局正则：`name = "tauri"` 这种短名会在别的
 * 包的 dependencies 列表里再次出现，贴着它往后找 version 会读到邻居的版本。
 */
export function extractCrateVersion(lockSrc, crate) {
  for (const block of lockSrc.split("[[package]]")) {
    const name = block.match(/^\s*name\s*=\s*"([^"]+)"/m);
    if (name?.[1] !== crate) continue;
    const version = block.match(/^\s*version\s*=\s*"([^"]+)"/m);
    if (!version) {
      throw new Error(`Cargo.lock 里 ${crate} 的块没有 version 字段`);
    }
    return version[1];
  }
  throw new Error(
    `Cargo.lock 里找不到 crate "${crate}" —— 依赖被移除时，` +
      "本守卫必须响亮失败而不是放行（配对表该同步删掉这一行）",
  );
}

/**
 * 从 pnpm-lock.yaml 的 importers 段里取某个 npm 包**实际解析到**的版本。
 *
 * 刻意读 lockfile 而不是 package.json 的 `^2` 这类范围：范围说明不了实际
 * 装了什么，而 `tauri build` 检查的是实际安装的版本。
 */
export function extractNpmVersion(lockSrc, pkg) {
  // importers 段形如：
  //   '@tauri-apps/plugin-log':
  //     specifier: ^2.9.0
  //     version: 2.9.0
  const escaped = pkg.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const re = new RegExp(
    `['"]?${escaped}['"]?:\\s*\\n\\s*specifier:[^\\n]*\\n\\s*version:\\s*([^\\s(]+)`,
  );
  const m = lockSrc.match(re);
  if (!m) {
    throw new Error(
      `pnpm-lock.yaml 里找不到 npm 包 "${pkg}" 的解析版本 —— ` +
        "依赖被移除或 lockfile 未更新（先跑一次 pnpm install）",
    );
  }
  return m[1];
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkTauriParity({ cargoLock, pnpmLock, pairs }) {
  const errors = [];
  for (const { crate, npm } of pairs) {
    const crateVersion = extractCrateVersion(cargoLock, crate);
    const npmVersion = extractNpmVersion(pnpmLock, npm);
    const a = majorMinor(crateVersion, crate);
    const b = majorMinor(npmVersion, npm);
    if (a !== b) {
      errors.push(
        `Tauri 版本不一致：${crate} (Rust) = ${crateVersion} → ${a}，` +
          `${npm} (npm) = ${npmVersion} → ${b} —— ` +
          "major.minor 必须相同，否则 `tauri build` 会在 release 构建腿上直接拒绝构建",
      );
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一：symlink 调用下裸比较不相等 → 守卫静默 exit 0，比没有守卫更糟。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const pairs = derivePairs(readFileSync(join(root, "package.json"), "utf8"));
  const errors = checkTauriParity({
    cargoLock: readFileSync(join(root, "Cargo.lock"), "utf8"),
    pnpmLock: readFileSync(join(root, "pnpm-lock.yaml"), "utf8"),
    pairs,
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\ntauri parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log(`✓ tauri parity OK — ${pairs.length} 对 Rust crate / npm 包的 major.minor 逐对一致`);
}
