import { defineConfig } from "vite";
import solid from "vite-plugin-solid";

const aqx = `http://127.0.0.1:${process.env.AQX_PORT ?? "4970"}`;

export default defineConfig({
  plugins: [solid()],
  server: {
    host: "127.0.0.1",
    allowedHosts: [".ts.net"],
    proxy: { "/api": { target: aqx } },
  },
  build: { target: "es2022", assetsInlineLimit: 0 },
});
