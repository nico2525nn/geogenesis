import { defineConfig } from "vite";

// The engine is a plain WebAssembly module (no wasm-bindgen), loaded by URL
// from /public. `worker.format = "es"` keeps the worker a module worker so it
// can `import` the shared types and loader.
// GitHub Pages serves the demo from a repository sub-path; CI sets VITE_BASE
// (e.g. "/geogenesis/") while local dev/preview keeps "/".
export default defineConfig({
  base: process.env.VITE_BASE ?? "/",
  server: { port: 4000, strictPort: true },
  worker: { format: "es" },
  build: { target: "es2022", outDir: "dist", sourcemap: false },
});
