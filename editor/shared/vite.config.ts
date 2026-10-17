import { resolve } from "node:path";
import { defineConfig } from "vite";
import { SHARED_MODULES, externalSharedModules } from "./plugin.ts";

const entry = (name: string) => resolve(import.meta.dirname, `src/${name}.js`);

// The modules plugins share with the editor, one ES module each under `dist/shared/`. Each
// keeps the other shared modules external, so all of them use the one `react`.
export default defineConfig({
  plugins: [externalSharedModules()],
  define: { "process.env.NODE_ENV": JSON.stringify("production") },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    lib: {
      entry: Object.fromEntries(Object.values(SHARED_MODULES).map((name) => [name, entry(name)])),
      formats: ["es"],
      fileName: (_, name) => `shared/${name}.js`,
    },
    rolldownOptions: {
      output: { chunkFileNames: "shared/[name]-[hash].js" },
    },
  },
});
