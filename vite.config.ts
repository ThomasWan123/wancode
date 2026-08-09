import { defineConfig, type Plugin } from "vite";
import react from "@vitejs/plugin-react";

const MAX_JS_CHUNK_BYTES = 850 * 1024;

function bundleBudget(): Plugin {
  return {
    name: "wancode-bundle-budget",
    generateBundle(_options, bundle) {
      for (const output of Object.values(bundle)) {
        if (output.type !== "chunk") continue;
        const bytes = new TextEncoder().encode(output.code).byteLength;
        if (bytes > MAX_JS_CHUNK_BYTES) {
          this.error(
            `${output.fileName} is ${bytes} bytes; WanCode's per-chunk budget is ${MAX_JS_CHUNK_BYTES} bytes`,
          );
        }
      }
    },
  };
}

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react(), bundleBudget()],

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  build: {
    // The custom plugin above makes this a hard gate. Keep Vite's display
    // threshold aligned so a passing build does not emit a contradictory warning.
    chunkSizeWarningLimit: MAX_JS_CHUNK_BYTES / 1024,
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
