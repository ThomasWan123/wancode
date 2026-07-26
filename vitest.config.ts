import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    // 组件测试需要 DOM。纯逻辑测试（modelBlock.test.ts）在 jsdom 下同样能跑，
    // 不必为它们单开一套环境。
    environment: "jsdom",
    setupFiles: ["./src/test-setup.ts"],
  },
});
