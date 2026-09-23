/// <reference types="vitest/config" />

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true
  },
  envPrefix: ["VITE_", "TAURI_ENV_*"],
  build: {
    target: process.env.TAURI_ENV_PLATFORM === "windows" ? "chrome105" : "safari13",
    minify: !process.env.TAURI_ENV_DEBUG ? "esbuild" : false,
    sourcemap: Boolean(process.env.TAURI_ENV_DEBUG)
  },
  test: {
    environment: "jsdom",
    setupFiles: "./src/test/setup.ts",
    css: true,
    // .scratch 下可能存放其它工作区副本，避免把别人的测试当成本仓库测试执行。
    exclude: ["**/node_modules/**", "**/.scratch/**", "**/dist/**"],
    // 测试并发跑在真实 SQLite/原生构建同时进行的机器上，5s 默认值会因负载抖动误报；
    // 这里放宽到 15s，断言内容不放宽。
    testTimeout: 15000
  }
});
