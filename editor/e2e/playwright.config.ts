import { defineConfig, devices } from "@playwright/test";
import { launchArgs } from "./project";

const port = 4310;
/** The renderer's pixel tests, run once per browser backend; every other spec runs once. */
const renderer = /renderer\.spec\.ts$/;
/** The renderer's tests that only WebGPU needs. */
const webgpu = /renderer-webgpu\.spec\.ts$/;

export default defineConfig({
  testDir: ".",
  // The golden check runs 10,000 ticks of a release build in the worker, and the first spec
  // waits for the first build (`FIRST_BUILD_MS` in project.ts).
  timeout: (process.env.CI ? 20 : 10) * 60 * 1000,
  // The specs share one cargo target directory, and the performance spec measures frame times:
  // one at a time.
  workers: 1,
  expect: { timeout: 15_000 },
  use: {
    actionTimeout: 30_000,
    ...devices["Desktop Chrome"],
    baseURL: `http://localhost:${port}`,
    launchOptions: { args: launchArgs },
  },
  projects: [
    { name: "chromium", testIgnore: [renderer, webgpu] },
    // Without `--enable-unsafe-webgpu`, headless Chromium offers no WebGPU adapter, so the
    // renderer draws with WebGL2 (SwiftShader through ANGLE).
    { name: "webgl2", testMatch: renderer },
    // With it, headless Chromium offers SwiftShader's WebGPU adapter (Vulkan in software), on
    // macOS and on Linux runners alike: no GPU needed.
    { name: "webgpu", testMatch: [renderer, webgpu], use: { launchOptions: { args: [...launchArgs, "--enable-unsafe-webgpu"] } } },
  ],
  webServer: {
    command: `vite --config e2e/vite.config.ts --port ${port} --strictPort`,
    cwd: "..",
    url: `http://localhost:${port}`,
    reuseExistingServer: !process.env.CI,
  },
});
