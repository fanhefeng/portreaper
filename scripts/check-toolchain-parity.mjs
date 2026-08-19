#!/usr/bin/env node
// Rust 工具链版本的跨清单一致性校验。
//
// 为什么需要：工具链版本写在三处 —— rust-toolchain.toml 的 channel（约束本机
// rustup 与 tauri-action），以及 ci.yml / release.yml 里 dtolnay/rust-toolchain
// 步骤的 `toolchain:` 输入（action 不读 toml，版本要在 workflow 里重写一遍）。
// 三处此前只靠 rust-toolchain.toml 头部「升级流程」注释人肉同步。漏改一处的
// 症状是本机与 CI 的 rustfmt 启发式分叉、`cargo fmt --check` 在其中一边翻红 ——
// 钉版本（rust-toolchain.toml 的存在意义）钉不齐，等于没钉。
//
// 口径：workflow 里**每一条** `toolchain:` 都必须等于 toml 的 channel —— 将来
// 矩阵腿变多时新增的行自动纳管；一条都找不到时响亮失败（步骤被改名/移除时，
// 守卫静默放行比没有守卫更危险，同 check-reason-parity）。
//
// 用法：node scripts/check-toolchain-parity.mjs   （exit 1 = 不一致）
// 自测：node --test scripts/*.test.mjs

import { readFileSync, realpathSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

/**
 * 提取 rust-toolchain.toml 的 `channel = "..."`。
 *
 * 先剔除注释行：升级时被注释掉留作参考的旧 channel 排在真值之前会被 match
 * 先命中（同 check-paths-parity 的 codeLines 处理）。找不到就抛，绝不返回
 * undefined 让两侧「同为 undefined 而相等」地静默通过。
 */
export function extractToolchainChannel(tomlSrc) {
  const code = tomlSrc
    .split("\n")
    .filter((l) => !l.trim().startsWith("#"))
    .join("\n");
  const m = code.match(/^\s*channel\s*=\s*"([^"]+)"/m);
  if (!m) {
    throw new Error(
      '在 rust-toolchain.toml 里找不到 `channel = "..."` —— ' +
        "字段被改名或改形态时，本守卫必须响亮失败而不是放行",
    );
  }
  return m[1];
}

/**
 * 提取一个 workflow 文件里全部 `toolchain:` 输入的值（支持裸值与引号值，
 * 忽略整行注释与行尾注释）。空结果直接抛：dtolnay/rust-toolchain 步骤被
 * 移除或换写法时，守卫要喊出来而不是「没找到 = 没问题」。
 *
 * **按「安装步骤」计数，而不只是扫 `toolchain:` 行**（评审发现）：本守卫的口径是
 * 「workflow 里每一条 toolchain 都等于 toml 的 channel」，而
 * `uses: dtolnay/rust-toolchain@stable` 这种**不带 `toolchain:` 输入**的写法压根
 * 不会出现在扫描结果里 —— 加一个这样的 job，守卫返回 0 个错误。实际危害目前被
 * `rust-toolchain.toml` 兜住（rustup 在仓库内覆盖 default），但那是运气，不是守卫。
 */
export function extractWorkflowToolchains(yamlSrc, label) {
  const values = [];
  let installSteps = 0;
  for (const line of yamlSrc.split("\n")) {
    if (line.trim().startsWith("#")) continue;
    if (/^\s*(-\s*)?uses:\s*["']?dtolnay\/rust-toolchain/.test(line)) installSteps += 1;
    const m = line.match(/^\s*toolchain:\s*["']?([^\s"'#]+)/);
    if (m) values.push(m[1]);
  }
  if (values.length === 0) {
    throw new Error(
      `在 ${label} 里找不到任何 \`toolchain:\` 输入 —— ` +
        "安装步骤被改名/移除时，本守卫必须响亮失败而不是放行",
    );
  }
  // 用 `>` 而非 `!==`：要抓的是「有安装步骤没写 toolchain」这一个方向。
  // 反向（toolchain 行多于 uses 行）在片段输入下是常态，且本身无害。
  if (installSteps > values.length) {
    throw new Error(
      `${label} 有 ${installSteps} 个 dtolnay/rust-toolchain 步骤，却只有 ` +
        `${values.length} 条 \`toolchain:\` 输入 —— 不带该输入的步骤会静默用上 action ` +
        "默认的工具链，而本守卫看不见它；请给每个安装步骤显式写上版本",
    );
  }
  return values;
}

/** 核心校验（纯函数，可测）：返回错误信息数组（空 = 通过）。 */
export function checkToolchainParity({ tomlSrc, ciSrc, releaseSrc }) {
  const errors = [];
  const channel = extractToolchainChannel(tomlSrc);
  for (const [label, src] of [
    [".github/workflows/ci.yml", ciSrc],
    [".github/workflows/release.yml", releaseSrc],
  ]) {
    for (const v of extractWorkflowToolchains(src, label)) {
      if (v !== channel) {
        errors.push(
          `Rust 工具链版本不一致：rust-toolchain.toml channel = "${channel}"，` +
            `${label} toolchain: ${v} —— 本机与 CI 的 rustfmt/clippy 口径将分叉`,
        );
      }
    }
  }
  return errors;
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一：symlink 调用下裸比较不相等 → 守卫静默 exit 0，比没有守卫更糟。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const errors = checkToolchainParity({
    tomlSrc: readFileSync(join(root, "rust-toolchain.toml"), "utf8"),
    ciSrc: readFileSync(join(root, ".github/workflows/ci.yml"), "utf8"),
    releaseSrc: readFileSync(join(root, ".github/workflows/release.yml"), "utf8"),
  });
  if (errors.length > 0) {
    for (const e of errors) console.error(`✗ ${e}`);
    console.error(`\ntoolchain parity check FAILED (${errors.length}).`);
    process.exit(1);
  }
  console.log("✓ toolchain parity OK — rust-toolchain.toml 与两个 workflow 的版本一致");
}
