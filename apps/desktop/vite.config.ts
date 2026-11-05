import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

// Tauri expects a fixed port and no clear-screen, so its own logs stay readable.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true, host: '127.0.0.1' },
  build: { target: 'safari16' },
})
