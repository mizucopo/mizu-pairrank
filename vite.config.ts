import { defineConfig } from "vite";

const tauriDevHost = process.env.TAURI_DEV_HOST;
const server = tauriDevHost
  ? {
      hmr: {
        host: tauriDevHost,
        port: 1421,
        protocol: "ws" as const,
      },
      host: tauriDevHost,
      port: 1420,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    }
  : {
      host: false,
      port: 1420,
      strictPort: true,
      watch: {
        ignored: ["**/src-tauri/**"],
      },
    };

export default defineConfig({
  clearScreen: false,
  server,
});
