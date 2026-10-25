import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  // Cargo locks binaries under src-tauri/target while linking; watching them crashes Vite on Windows.
  server: { port: 1437, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  envPrefix: ["VITE_"],
  build: { target: "es2022" },
});
