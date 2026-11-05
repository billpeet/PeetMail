import { defineConfig } from "drizzle-kit";

// `pnpm db:generate` writes a migration to drizzle/ after a change to the schema.
export default defineConfig({
  dialect: "sqlite",
  schema: "./src/lib/server/schema.ts",
  out: "./drizzle",
});
