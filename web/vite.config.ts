import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const mira = `http://127.0.0.1:${process.env.MIRA_PORT ?? "4970"}`;

export default defineConfig({
  plugins: [solid()],
  server: {
    host: "127.0.0.1",
    allowedHosts: [".ts.net"],
    proxy: { "/api": { target: mira } },
  },
  build: { target: "es2022", assetsInlineLimit: 0 },
});
