import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

// vitest 未开 globals，RTL 的自动清理不会注册——不清的话各用例的 DOM 会
// 累积在同一个 document 里，查询到的是上一个用例留下的节点。
afterEach(cleanup);
