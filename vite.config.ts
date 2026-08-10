import { createRequire } from "node:module";
import { defineConfig, lazyPlugins } from "vite-plus";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

// 版本号编译期注入（footer 展示用）。取自 package.json 而不是再开一个来源：
// `scripts/bump-version.mjs` 已经在同步它，与 tauri.conf.json / Cargo.toml 同源，
// 不会漂移；`node scripts/bump-version.mjs --check` 也覆盖到这一份。
// 用 createRequire 而非 import assertion：本文件在 vite-plus 下按 ESM 加载，
// JSON import 的语法在各 Node/TS 版本间还不稳定。
const appVersion = createRequire(import.meta.url)("./package.json").version as string;

export default defineConfig(async () => ({
  plugins: lazyPlugins(() => [react()]),

  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },

  // pre-commit（.vite-hooks/pre-commit → `vp staged`）对暂存文件跑 fmt+lint+类型检查。
  // 函数形式的 config 让 `vp migrate` 无法自动合并，故手动维护（见 viteplus.dev/guide/migrate）。
  // json/yml/toml 也在列（评审根因）：bump-version 重写过的 tauri.conf.json
  // 曾因 glob 只有 js/ts 而绕过本钩子，格式回退直到 CI 才被拦下。
  // *.rs 同理（v0.7.2 事故）：`vp check` 只管 JS/TS 侧，Rust 格式在本地一度
  // 零门禁，v0.6.0 与 v0.7.2 两次都是 `cargo fmt --check` 在 CI 上翻红。这里
  // 用 rustfmt 而非 cargo fmt —— staged 会把暂存文件名追加到命令末尾，
  // rustfmt 直接吃文件列表，cargo fmt 不吃。edition 须与各 crate 的 Cargo.toml
  // 同步（crates/portreaper-core 与 src-tauri，目前都是 2021）。
  // "*.rs" 不含 `/`，故按 basename 匹配任意深度 —— crates/ 下的引擎源码同样覆盖。
  // css/html 同在 oxfmt 的检查范围（0.2.6 实测会报格式错）：glob 缺它们时，
  // 只改 index.html / *.css 的提交在 pre-commit 层拿不到自动修复。
  staged: {
    "*.{js,mjs,ts,tsx,json,yml,yaml,toml,css,html}": "vp check --fix",
    "*.rs": "rustfmt --edition 2021",
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    // 用 1430/1431 而非 Tauri 脚手架默认的 1420/1421：默认值会与本机其它
    // Tauri 项目（同样默认 1420）抢端口，strictPort 下直接启动失败。固定错开。
    port: 1430,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1431,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching Rust sources (`src-tauri` = GUI shell,
      //    `crates` = portreaper-core engine) — cargo owns their rebuilds.
      ignored: ["**/src-tauri/**", "**/crates/**"],
    },
  },
}));
