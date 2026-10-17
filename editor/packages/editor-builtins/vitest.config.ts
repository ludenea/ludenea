import { defineConfig } from "vitest/config";

export default defineConfig({
  // The physics crate's engine plugin, tested here, sits outside any package: resolve the shared
  // modules it imports from this package.
  resolve: { dedupe: ["react", "@ludenea/editor-shell", "@ludenea/control-client"] },
  test: { name: "editor-builtins", environment: "jsdom" },
});
