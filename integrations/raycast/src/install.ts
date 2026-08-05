/**
 * portreaper-cli 的自动获取。
 *
 * # 为什么必须自动下载
 *
 * Raycast Store 对依赖外部二进制的扩展有明确规定：允许「从可信源下载并校验完整性」，
 * 但**不允许**把安装工作丢给用户（"Avoid asking users to perform additional downloads
 * and try to automate as much as possible from the extension"）。所以扩展不能只是
 * 提示「请先 cargo install」，必须自己把二进制取回来。
 *
 * # 为什么不把二进制打进扩展包
 *
 * 同一份规则反对 "heavy binary bundling"：三个平台的二进制会让每个用户的扩展下载
 * 体积翻好几倍，而其中两份他永远用不到。
 *
 * # 完整性校验
 *
 * 先取 `portreaper-cli-SHA256SUMS`（由 release 流水线在 publish 阶段汇总三条构建腿
 * 的产物生成），再据它核对下载到的二进制。校验不通过就**删除**文件并报错 ——
 * 一个来路不明的可执行文件绝不能留在磁盘上，更不能去执行它。
 */

import { createHash } from "node:crypto";
import { chmod, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import { join } from "node:path";

/** 稳定资产名 —— 与 `.github/workflows/release.yml` 的 matrix.cli_asset 一一对应。
 *  改名即 404，由 `scripts/check-release-assets.mjs` 守住两侧一致。 */
export const CLI_ASSETS = {
  "darwin-arm64": "portreaper-cli-macos-arm64",
  "darwin-x64": "portreaper-cli-macos-x64",
  "win32-x64": "portreaper-cli-windows-x64.exe",
} as const;

export const CHECKSUM_ASSET = "portreaper-cli-SHA256SUMS";

const RELEASE_BASE = "https://github.com/fanhefeng/portreaper/releases/latest/download";

export class UnsupportedPlatformError extends Error {
  constructor(key: string) {
    super(`No portreaper-cli build for ${key}`);
    this.name = "UnsupportedPlatformError";
  }
}

export class ChecksumMismatchError extends Error {
  constructor(
    readonly expected: string,
    readonly actual: string,
  ) {
    super("Downloaded binary failed its checksum — refusing to run it.");
    this.name = "ChecksumMismatchError";
  }
}

/** 当前平台对应的资产名。不支持的平台响亮失败，绝不猜一个名字去下载。 */
export function assetNameFor(platform: string, arch: string): string {
  const key = `${platform}-${arch}`;
  const name = (CLI_ASSETS as Record<string, string | undefined>)[key];
  if (!name) throw new UnsupportedPlatformError(key);
  return name;
}

/** 从 `sha256sum` 格式的清单里取出某个文件的期望哈希。 */
export function parseChecksums(text: string, assetName: string): string | undefined {
  for (const line of text.split("\n")) {
    // 格式：`<64 位十六进制>  <文件名>`（sha256sum 用两个空格）
    const m = line.trim().match(/^([0-9a-f]{64})\s+\*?(.+)$/i);
    if (m && m[2] === assetName) return m[1].toLowerCase();
  }
  return undefined;
}

export function sha256(buf: Buffer): string {
  return createHash("sha256").update(buf).digest("hex");
}

async function fetchBuffer(url: string): Promise<Buffer> {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) {
    throw new Error(`GET ${url} → HTTP ${res.status}`);
  }
  return Buffer.from(await res.arrayBuffer());
}

/** 已下载副本的落点（Raycast 给扩展的可写目录）。 */
export function installedCliPath(supportPath: string): string {
  const exe = process.platform === "win32" ? "portreaper-cli.exe" : "portreaper-cli";
  return join(supportPath, "bin", exe);
}

export function isInstalled(supportPath: string): boolean {
  return existsSync(installedCliPath(supportPath));
}

/**
 * 下载最新 release 的 CLI 到扩展的 supportPath，校验 sha256 后置为可执行。
 * 返回可执行文件路径。
 *
 * 任何一步失败都不会留下半个文件：校验失败会删除已落盘的二进制。
 */
export async function installCli(
  supportPath: string,
  onProgress?: (step: string) => void,
): Promise<string> {
  const assetName = assetNameFor(process.platform, process.arch);

  onProgress?.("Fetching checksums…");
  const sumsText = (await fetchBuffer(`${RELEASE_BASE}/${CHECKSUM_ASSET}`)).toString("utf8");
  const expected = parseChecksums(sumsText, assetName);
  if (!expected) {
    // 清单里没有这个资产 —— 可能是 release 不完整，也可能是资产改了名。
    // 无论哪种，都不能跳过校验继续装。
    throw new Error(`${CHECKSUM_ASSET} has no entry for ${assetName}`);
  }

  onProgress?.("Downloading portreaper-cli…");
  const bin = await fetchBuffer(`${RELEASE_BASE}/${assetName}`);

  onProgress?.("Verifying…");
  const actual = sha256(bin);
  if (actual !== expected) {
    throw new ChecksumMismatchError(expected, actual);
  }

  const dest = installedCliPath(supportPath);
  await mkdir(join(supportPath, "bin"), { recursive: true });
  await writeFile(dest, bin);
  try {
    await chmod(dest, 0o755);
  } catch (e) {
    await rm(dest, { force: true });
    throw e;
  }
  return dest;
}

/** 读回已安装二进制并复核哈希（用于「我这份是不是被换掉了」的排查）。 */
export async function verifyInstalled(supportPath: string): Promise<boolean> {
  const p = installedCliPath(supportPath);
  if (!existsSync(p)) return false;
  const assetName = assetNameFor(process.platform, process.arch);
  const sumsText = (await fetchBuffer(`${RELEASE_BASE}/${CHECKSUM_ASSET}`)).toString("utf8");
  const expected = parseChecksums(sumsText, assetName);
  if (!expected) return false;
  return sha256(await readFile(p)) === expected;
}
