import { readFile } from "node:fs/promises";
import { join, resolve, sep } from "node:path";
import { defineConfig, type Plugin } from "vite";

const repository = resolve(import.meta.dirname, "../..");

/** Serves the files of `directory` under the URL `prefix`, read-only. */
function mount(prefix: string, directory: string): Plugin {
  return {
    name: `mount ${prefix}`,
    configureServer(server) {
      server.middlewares.use(prefix, (request, response, next) => {
        const file = join(directory, decodeURIComponent((request.url ?? "/").split("?")[0] ?? "/"));
        if (!file.startsWith(directory + sep)) return next();
        readFile(file).then((bytes) => response.end(bytes), () => next());
      });
    },
  };
}

export default defineConfig({
  root: resolve(import.meta.dirname, "golden"),
  plugins: [
    mount("/assets", join(repository, "examples/boxes/assets")),
    mount("/golden", join(repository, "golden")),
  ],
});
