import { defineConfig } from "vitest/config";

export default defineConfig({ test: { name: "editor-builtins", environment: "jsdom" } });
