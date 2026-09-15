import { defineConfig } from "vite";

// Tauri 前端：静态单页，固定端口供 devUrl 使用
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
  },
});
