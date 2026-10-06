import { defineConfig } from "vite";

export default defineConfig({
  server: { port: 4000 },
  build: { target: "es2022", outDir: "dist" },
  worker: { format: "es" },
});
