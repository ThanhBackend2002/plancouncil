import { defineConfig } from "vitest/config";

// Config riêng cho test: không nạp plugin React/Tailwind của vite.config.ts, chỉ chạy logic thuần TS.
export default defineConfig({
  test: {
    environment: "node",
    include: ["src/**/*.test.ts"],
  },
});
