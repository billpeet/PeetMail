import { defineConfig } from "vitest/config";
import { sveltekit } from "@sveltejs/kit/vite";
import adapter from "@sveltejs/adapter-node";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [
    tailwindcss(),
    sveltekit({
      preprocess: vitePreprocess(),
      adapter: adapter(),
      // SvelteKit's CSRF check takes any POST/PUT/DELETE without a JSON body (a bodyless
      // DELETE, say) for a form submission and refuses it unless its Origin matches. Every
      // call here is authenticated by a bearer token, never a cookie, so there's nothing for
      // CSRF to ride on; and PeetMail sends no Origin. Revisit if a web client uses cookies.
      csrf: { trustedOrigins: ["*"] },
    }),
  ],
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
