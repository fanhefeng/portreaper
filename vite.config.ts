import { defineConfig, lazyPlugins } from "vite-plus";
import react from "@vitejs/plugin-react";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: lazyPlugins(() => [react()]),

  // pre-commit（.vite-hooks/pre-commit → `vp staged`）对暂存文件跑 fmt+lint+类型检查。
  // 函数形式的 config 让 `vp migrate` 无法自动合并，故手动维护（见 viteplus.dev/guide/migrate）。
  // json/yml/toml 也在列（评审根因）：bump-version 重写过的 tauri.conf.json
  // 曾因 glob 只有 js/ts 而绕过本钩子，格式回退直到 CI 才被拦下。
  // *.rs 同理（v0.7.2 事故）：`vp check` 只管 JS/TS 侧，Rust 格式在本地一度
  // 零门禁，v0.6.0 与 v0.7.2 两次都是 `cargo fmt --check` 在 CI 上翻红。这里
  // 用 rustfmt 而非 cargo fmt —— staged 会把暂存文件名追加到命令末尾，
  // rustfmt 直接吃文件列表，cargo fmt 不吃。edition 须与 src-tauri/Cargo.toml 同步。
  staged: {
    "*.{js,mjs,ts,tsx,json,yml,yaml,toml}": "vp check --fix",
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
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
