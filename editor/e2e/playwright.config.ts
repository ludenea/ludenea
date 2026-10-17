import { defineConfig, devices } from "@playwright/test";

const port = 4310;

export default defineConfig({
  testDir: ".",
  // The golden check runs 10,000 ticks of a release build in the worker.
  timeout: 10 * 60 * 1000,
  use: {
    ...devices["Desktop Chrome"],
    baseURL: `http://localhost:${port}`,
    launchOptions: { args: ["--use-angle=swiftshader"] },
  },
  webServer: {
    command: `vite --config e2e/vite.config.ts --port ${port} --strictPort`,
    cwd: "..",
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
  },
});
