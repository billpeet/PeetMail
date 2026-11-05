import { defineConfig } from "vite";
import { sveltekit } from "@sveltejs/kit/vite";
import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";
import process from "node:process";
const host = process.env.TAURI_DEV_HOST;

// https://v2.tauri.app/start/frontend/sveltekit/
export default defineConfig(() => ({
  plugins: [
    tailwindcss(),
    sveltekit({
      preprocess: vitePreprocess(),
      // Tauri has no Node server to do SSR, so adapter-static with an index.html fallback
      // puts the app in SPA mode. See https://svelte.dev/docs/kit/single-page-apps
      adapter: adapter({ fallback: "index.html" }),
      // The frontend ships inside the exe and updates through the Tauri updater, so there's
      // no deployment for SvelteKit's hourly version poll to find.
      version: { pollInterval: 0 },
    }),
  ],

  // Unit tests for pure TS modules. E2E lives in tests/e2e (Playwright).
  test: {
    include: ["src/**/*.test.ts", "scripts/**/*.test.ts"],
    environment: "node",
  },

  // Release builds (PEETMAIL_SOURCEMAPS=1 in release.yml) write source maps for Sentry,
  // "hidden" so the shipped JS doesn't point at them; CI uploads them, then deletes them
  // before they'd be embedded in the exe.
  build: { sourcemap: process.env.PEETMAIL_SOURCEMAPS === "1" ? /** @type {const} */ ("hidden") : false },

  // Don't hide Rust errors behind Vite's screen clearing.
  clearScreen: false,
  server: {
    // Tauri expects a fixed port.
    port: 1420,
    strictPort: true,
    host: host || "127.0.0.1",
    hmr: host ? { protocol: "ws", host, port: 1421 } : undefined,
    // .peetmail-dev: the dev build's database and WebView profile, written constantly.
    watch: { ignored: ["**/src-tauri/**", "**/.peetmail-dev/**", "**/.peetmail-test/**"] },
  },
}));
