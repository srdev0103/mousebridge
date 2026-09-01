import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Tauri sets these when it drives the build.
const host = process.env.TAURI_DEV_HOST;
const isWindows = process.env.TAURI_ENV_PLATFORM === "windows";
const isDebug = Boolean(process.env.TAURI_ENV_DEBUG);

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // Tauri owns the terminal output; clearing it hides Rust compiler errors.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    host: host ?? false,
    // Only set when serving to a remote device; `exactOptionalPropertyTypes`
    // forbids passing an explicit `undefined` here.
    ...(host ? { hmr: { protocol: "ws", host, port: 1421 } } : {}),
    // Rust changes are handled by cargo, not by Vite's watcher.
    watch: { ignored: ["**/src-tauri/**"] },
  },
  build: {
    // Match the oldest webview each platform ships: WebView2 on Windows 10,
    // WKWebView on macOS 13. Targeting higher silently breaks older hosts.
    target: isWindows ? "chrome105" : "safari16",
    minify: isDebug ? false : "esbuild",
    sourcemap: isDebug,
  },
});
