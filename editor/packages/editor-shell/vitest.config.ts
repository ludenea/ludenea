import { defineConfig } from "vitest/config";

export default defineConfig({ test: { name: "editor-shell", environment: "jsdom" } });
