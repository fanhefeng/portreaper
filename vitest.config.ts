import { createRequire } from "node:module";
import { defineConfig } from "vite-plus";
import react from "@vitejs/plugin-react";

// 前端回归测试（vitest + happy-dom）。与 vite.config.ts 分离：
// 测试不需要 Tauri 的固定端口 / HMR 约束。
//
// define 必须两边都写：vitest 不读 vite.config.ts，漏掉这份的话
// `__APP_VERSION__` 在测试里是未定义标识符，整个 App 渲染直接 ReferenceError。
// 两处同源（都读 package.json），不存在「值漂移」，只有「漏配」这一种失败模式。
const appVersion = createRequire(import.meta.url)("./package.json").version as string;

export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(appVersion),
  },
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
