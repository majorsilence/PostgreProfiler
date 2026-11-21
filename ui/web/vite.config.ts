import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'

// `npm run dev` forwards the API to a server on port 8080.
export default defineConfig({
  plugins: [vue()],
  server: { proxy: { '/api': 'http://localhost:8080' } },
  test: { environment: 'node' },
})
