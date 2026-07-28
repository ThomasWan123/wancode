/* v0.18.7-B 复核补测：RTL 直接喂 camelCase 对象，证明不了
   Tauri(snake_case) → parseModelOptions → Composer 这段真实链路。 */
import { describe, expect, it } from "vitest";
import { parseModelOptions } from "./modelOption";

describe("parseModelOptions", () => {
  it("接受 Tauri 侧真实的 snake_case 形状", () => {
    const out = parseModelOptions([
      { id: "glm-coding", name: "GLM Coding Plan", endpoint_label: "open.bigmodel.cn" },
    ]);
    expect(out).toEqual([
      { id: "glm-coding", name: "GLM Coding Plan", endpointLabel: "open.bigmodel.cn" },
    ]);
  });

  it("name 缺失回退为 id；endpoint 缺失为空串（显示层只出 name）", () => {
    const out = parseModelOptions([{ id: "solo" }]);
    expect(out).toEqual([{ id: "solo", name: "solo", endpointLabel: "" }]);
  });

  it("坏形状条目被丢弃，非数组当作空", () => {
    expect(parseModelOptions([{ name: "no-id" }, null, { id: "ok" }])).toHaveLength(1);
    expect(parseModelOptions("boom")).toEqual([]);
    expect(parseModelOptions(undefined)).toEqual([]);
  });
});
