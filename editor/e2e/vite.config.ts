import { readFile } from "node:fs/promises";
import { extname, join, resolve, sep } from "node:path";
import { defineConfig, type Plugin } from "vite";

const repository = resolve(import.meta.dirname, "../..");
/** The types a browser needs to run a module and stream a WebAssembly one; others go untyped. */
const TYPES: Record<string, string> = { ".html": "text/html", ".js": "text/javascript", ".wasm": "application/wasm" };

/** Serves the files of `directory` under the URL `prefix`, read-only. */
function mount(prefix: string, directory: string): Plugin {
  return {
    name: `mount ${prefix}`,
    configureServer(server) {
      server.middlewares.use(prefix, (request, response, next) => {
        let path: string;
        try {
          path = decodeURIComponent((request.url ?? "/").split("?")[0] ?? "/");
        } catch {
          response.statusCode = 400;
          return response.end();
        }
        const file = join(directory, path);
        if (!file.startsWith(directory + sep)) return next();
        readFile(file).then(
          (bytes) => {
            const type = TYPES[extname(file)];
            if (type) response.setHeader("Content-Type", type);
            response.end(bytes);
          },
          () => next(),
        );
      });
    },
  };
}

export default defineConfig({
  root: resolve(import.meta.dirname, "golden"),
  plugins: [
    mount("/assets", join(repository, "examples/boxes/assets")),
    mount("/golden", join(repository, "golden")),
    mount("/renderer", join(import.meta.dirname, "fixtures/renderer")),
  ],
});
