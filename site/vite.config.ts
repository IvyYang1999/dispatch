import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// A static page: relative asset paths, so it works at a domain root or under /dispatch/ on GitHub Pages.
export default defineConfig({
  base: './',
  plugins: [react()],
  build: { target: 'es2022' },
  server: { host: '127.0.0.1' },
  preview: { host: '127.0.0.1' },
  resolve: { dedupe: ['react', 'react-dom'] },
})
