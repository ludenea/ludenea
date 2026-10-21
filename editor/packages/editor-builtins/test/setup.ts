import { vi } from "vitest";

// jsdom has no canvas: its `getContext` logs "Not implemented" and returns null. The viewport
// draws only when it gets a context, so tests that draw give it one with `vi.spyOn`.
HTMLCanvasElement.prototype.getContext = () => null;

// The renderer's module is the fake (vitest.config.ts), one instance for every load: each new
// instance runs its `init` again, which the fake records.
vi.mock("../src/shared/render-module", async () => {
  const fake = await import("./fake-render-wasm");
  return {
    async instantiate() {
      await fake.default();
      return fake;
    },
  };
});
