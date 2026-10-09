import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import { fileURLToPath, URL } from 'node:url'

// Tauri 期望 frontendDist 指向 dist/
// dev 时 tauri 用 devUrl (http://localhost:1420)
export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./vue/src', import.meta.url))
    }
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: '127.0.0.1',
    hmr: { protocol: 'ws', host: '127.0.0.1', port: 1421 },
    watch: {
      // 避免 vite 监听 src-tauri 触发 Rust 反复 rebuild
      ignored: ['**/src-tauri/**']
    }
  },
  build: {
    target: 'es2021',
    outDir: 'dist',
    emptyOutDir: true
  }
})
