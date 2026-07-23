import { defineConfig } from "vite-plus";
import react from "@vitejs/plugin-react";

// 前端回归测试（vitest + happy-dom）。与 vite.config.ts 分离：
// 测试不需要 Tauri 的固定端口 / HMR 约束。
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "happy-dom",
    include: ["src/**/*.test.{ts,tsx}"],
  },
});
