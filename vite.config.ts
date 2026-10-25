import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { version } from "./package.json";
export default defineConfig({
  plugins: [react()],
  // Lets browser previews show the same version as the native app.
  define: { __APP_VERSION__: JSON.stringify(version) },
  clearScreen: false,
  // Cargo locks binaries under src-tauri/target while linking; watching them crashes Vite on Windows.
  server: { port: 1437, strictPort: true, watch: { ignored: ["**/src-tauri/**"] } },
  envPrefix: ["VITE_"],
  build: { target: "es2022" },
});
