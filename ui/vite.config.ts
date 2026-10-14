import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// Tauri 惯例：dev server 固定端口 1420

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // 不监听 src-tauri 目录
      ignored: ['**/src-tauri/**'],
    },
  },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: {
    // Tauri 使用较新的浏览器内核，无需为旧浏览器降低目标
    target: 'es2022',
    minify: 'esbuild',
    sourcemap: false,
  },
})
