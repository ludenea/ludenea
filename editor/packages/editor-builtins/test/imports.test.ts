/// <reference types="node" />
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

// Built-ins get no more than project plugins: the shared runtime modules, plus the list library
// and the renderer.
const ALLOWED = new Set([
  "react",
  "react/jsx-runtime",
  "react-dom",
  "@ludenea/editor-shell",
  "@ludenea/control-client",
  "@tanstack/react-virtual",
  "@ludenea/render-wasm",
  // The renderer's files, loaded as a new instance of its module after a panic.
  "@ludenea/render-wasm/ludenea_render.js?url",
  "@ludenea/render-wasm/ludenea_render_bg.wasm?url",
]);
const SPECIFIER = /\b(?:import|export)\b[^"'`;]*?\bfrom\s*["']([^"']+)["']|\bimport\s*\(?\s*["']([^"']+)["']/g;

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap((entry) =>
    entry.isDirectory() ? sources(join(directory, entry.name)) : [join(directory, entry.name)],
  );
}

describe("imports", () => {
  it("builtins_import_only_public_modules", () => {
    const files = sources(join(import.meta.dirname, "../src"));
    expect(files.length).toBeGreaterThan(4);
    const forbidden = files.flatMap((file) =>
      [...readFileSync(file, "utf8").matchAll(SPECIFIER)]
        .map((match) => match[1] ?? match[2]!)
        .filter((specifier) => !specifier.startsWith(".") && !ALLOWED.has(specifier))
        .map((specifier) => `${file}: ${specifier}`),
    );
    expect(forbidden).toEqual([]);
  });

  it("the_check_catches_each_import_form", () => {
    const text = `import a from "zustand";\nimport type { B } from "dockview-react";\nexport * from "node:fs";\nimport "side-effect";\nconst c = await import("lazy");\nimport { d } from "./local";`;
    expect([...text.matchAll(SPECIFIER)].map((match) => match[1] ?? match[2])).toEqual([
      "zustand",
      "dockview-react",
      "node:fs",
      "side-effect",
      "lazy",
      "./local",
    ]);
  });
});
