import { defineConfig, devices } from "@playwright/test";

const port = 4310;

export default defineConfig({
  testDir: ".",
  // The golden check runs 10,000 ticks of a release build in the worker.
  timeout: 10 * 60 * 1000,
  // The specs share one cargo target directory, and the performance spec measures frame times:
  // one at a time.
  workers: 1,
  expect: { timeout: 15_000 },
  use: {
    actionTimeout: 30_000,
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
