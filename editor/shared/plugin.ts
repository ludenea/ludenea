import { cpSync, readFileSync } from "node:fs";
import { join, resolve, sep } from "node:path";
import type { Plugin } from "vite";

/** Specifier -> file name under `shared/`, as the import map in each app's `index.html` lists them. */
export const SHARED_MODULES = {
  react: "react",
  "react/jsx-runtime": "react-jsx-runtime",
  "react-dom": "react-dom",
  "@ludenea/editor-shell": "editor-shell",
  // The applications' part of the shell: in the shared build so it shares the plugins' instance
  // of the shell's modules, but never a plugin's import (plugin bundles refuse it).
  "@ludenea/editor-shell/app": "editor-shell-app",
  "@ludenea/control-client": "control-client",
} as const;

const REQUIRED = "\0shared-required:";
const built = resolve(import.meta.dirname, "dist/shared");

/** A file of the shared build by its path under `shared/`, or null. */
function readShared(path: string): string | null {
  const file = join(built, decodeURIComponent(path));
  if (!file.startsWith(built + sep)) return null;
  try {
    return readFileSync(file, "utf8");
  } catch {
    return null;
  }
}

const isShared = (id: string): id is keyof typeof SHARED_MODULES => Object.hasOwn(SHARED_MODULES, id);

/**
 * Leaves the shared modules external. CommonJS bundled next to them (`react-dom/client`
 * requiring `react` and `react-dom`) would call `require` at runtime, which browsers lack: such a `require` gets an ES
 * module re-exporting the external one instead. On a development server, imports point at
 * the served `/shared/<name>.js` files, the URLs the import map gives plugins.
 */
export function externalSharedModules(): Plugin {
  return {
    name: "ludenea-external-shared-modules",
    // Before Vite's own resolver, which would find the packages in node_modules.
    enforce: "pre",
    resolveId(id, _importer, options) {
      if (!isShared(id)) return null;
      if (this.environment.mode === "dev") return { id: `/shared/${SHARED_MODULES[id]}.js`, external: true };
      return (options as { kind?: string }).kind === "require-call" ? REQUIRED + id : { id, external: true };
    },
    load(id) {
      return id.startsWith(REQUIRED) ? `export * from ${JSON.stringify(id.slice(REQUIRED.length))};` : null;
    },
  };
}

/**
 * Keeps the shared modules out of an app: its build leaves them external (the import map in
 * its `index.html` resolves them) and gets a copy of `dist/shared/`; its development server
 * serves that directory, so `vite dev` also runs one React. Build `editor/shared` first.
 */
export function sharedModules(): Plugin[] {
  return [
    externalSharedModules(),
    {
      name: "ludenea-serve-shared-modules",
      config: () => ({
        optimizeDeps: { exclude: Object.keys(SHARED_MODULES) },
        // The shared `react/jsx-runtime` is the production one, without `jsxDEV`.
        oxc: { jsx: { runtime: "automatic", development: false } },
      }),
      configureServer(server) {
        server.middlewares.use("/shared", (request, response, next) => {
          const code = readShared((request.url ?? "/").split("?")[0] ?? "/");
          if (code === null) return next();
          response.setHeader("Content-Type", "text/javascript");
          response.end(code);
        });
      },
      // Requests get the files from the middleware above, unchanged. This only quiets the
      // server's warm-up of the imports it saw, which would otherwise look for them on disk.
      load: (id) => (id.startsWith("/shared/") ? "" : null),
      writeBundle(options) {
        cpSync(built, join(options.dir!, "shared"), { recursive: true });
      },
    },
  ];
}
