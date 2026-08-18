#!/usr/bin/env node
// 生成应用内更新的静态源 latest.json（tauri-plugin-updater 的 Static JSON 格式）。
//
// 由 release.yml 的 publish job 调用，**取代** tauri-action 自带的 uploadUpdaterJson：
// 三条 build 腿并行往同一个 release 合并 latest.json 是 tauri-action 的已知竞态
// （读-改-写无锁，可能互相覆盖掉对方的平台条目）。publish job 在全部资产就位后
// 一次性生成，内容确定、平台齐全性在这里响亮校验 —— 缺一个平台就拒绝生成，
// 而不是发布一份「装了 Windows 却更新不了 macOS」的静默残缺源。
//
// 用法：node scripts/generate-latest-json.mjs <tag> <sigs-dir> [notes]   （输出到 stdout）
//   <sigs-dir> 放着 `gh release download --pattern '*.sig'` 下来的签名文件，
//   资产名 = 签名文件名去掉 .sig 后缀（tauri 的 .sig 与被签资产同名成对）。
// 自测：node --test scripts/*.test.mjs           （generate-latest-json.test.mjs）

import { readdirSync, readFileSync, realpathSync } from "node:fs";
import { basename, join } from "node:path";
import { pathToFileURL } from "node:url";

/**
 * 资产名 → updater 平台键（OS-ARCH，tauri 端按运行时 target 取键）。
 * 模式与 release.yml verify 步骤的 glob 同源：tauri-action 上传 macOS updater
 * 包为 `<name>_<ver>_{aarch64,x64}.app.tar.gz`，Windows 直接复用 NSIS 安装器。
 */
export const PLATFORM_PATTERNS = [
  { key: "darwin-aarch64", re: /_aarch64\.app\.tar\.gz$/ },
  { key: "darwin-x86_64", re: /_x64\.app\.tar\.gz$/ },
  { key: "windows-x86_64", re: /_x64-setup\.exe$/ },
];

/** 资产名白名单：进 URL 的只允许保守字符集（GitHub 资产名本就如此，
 *  这里挡的是「意外混进一个奇怪文件」而不是恶意输入）。 */
const SAFE_NAME_RE = /^[A-Za-z0-9._-]+$/;

/**
 * 纯函数：由 (tag, 资产 + 签名列表) 生成 latest.json 对象。
 *
 * @param {object} args
 * @param {string} args.tag - 形如 "v0.11.0"（版本号 = 去掉前缀 v）
 * @param {string} args.repo - "owner/repo"
 * @param {{name: string, sig: string}[]} args.assets - 资产名 + .sig 文件内容
 * @param {string} [args.notes]
 * @param {string} args.pubDate - RFC 3339（CLI 入口传 now；参数化以便自测）
 */
export function generateLatestJson({ tag, repo, assets, notes = "", pubDate }) {
  if (!/^v\d/.test(tag)) throw new Error(`tag "${tag}" 不是 vX.Y.Z 形态`);
  if (!/^[\w.-]+\/[\w.-]+$/.test(repo)) throw new Error(`repo "${repo}" 不是 owner/repo 形态`);

  const platforms = {};
  for (const { name, sig } of assets) {
    if (!SAFE_NAME_RE.test(name)) throw new Error(`资产名 "${name}" 含意外字符`);
    const matches = PLATFORM_PATTERNS.filter((p) => p.re.test(name));
    if (matches.length !== 1) {
      throw new Error(
        `资产 "${name}" 命中 ${matches.length} 个平台模式（应恰好 1 个）—— ` +
          "要么混入了预期外的 .sig，要么 tauri 打包命名变了（连同 release.yml 的 verify 一起改）",
      );
    }
    const key = matches[0].key;
    if (platforms[key])
      throw new Error(`平台 ${key} 出现了两个资产：${platforms[key].url} 与 ${name}`);
    const signature = sig.trim();
    if (!signature) throw new Error(`资产 "${name}" 的签名内容为空`);
    platforms[key] = {
      signature,
      url: `https://github.com/${repo}/releases/download/${tag}/${name}`,
    };
  }

  const missing = PLATFORM_PATTERNS.map((p) => p.key).filter((k) => !platforms[k]);
  if (missing.length > 0) {
    throw new Error(
      `缺平台：${missing.join(", ")} —— 拒绝生成残缺的 latest.json` +
        "（对应平台的用户会静默失去应用内更新）",
    );
  }

  return {
    version: tag.replace(/^v/, ""),
    notes,
    pub_date: pubDate,
    platforms,
  };
}

// ---- CLI 入口（被 import 时不执行）----
// realpath 双侧归一：symlink 调用下裸比较不相等 → 静默 exit 0（与各守卫脚本同一教训）。
if (process.argv[1] && pathToFileURL(realpathSync(process.argv[1])).href === import.meta.url) {
  const [tag, sigsDir, notes = ""] = process.argv.slice(2);
  if (!tag || !sigsDir) {
    console.error("用法：node scripts/generate-latest-json.mjs <tag> <sigs-dir> [notes]");
    process.exit(1);
  }
  const repo = process.env.GH_REPO;
  if (!repo) {
    console.error("需要 GH_REPO 环境变量（owner/repo）—— release.yml 的 publish job 已导出它");
    process.exit(1);
  }
  const assets = readdirSync(sigsDir)
    .filter((f) => f.endsWith(".sig"))
    .map((f) => ({
      name: basename(f, ".sig"),
      sig: readFileSync(join(sigsDir, f), "utf8"),
    }));
  const json = generateLatestJson({
    tag,
    repo,
    assets,
    notes,
    pubDate: new Date().toISOString(),
  });
  process.stdout.write(`${JSON.stringify(json, null, 2)}\n`);
}
