/* v0.18.7-B 复核补测：RTL 直接喂 camelCase 对象，证明不了
   Tauri(snake_case) → parseModelOptions → Composer 这段真实链路。 */
import { describe, expect, it } from "vitest";
import {
  effortStateForModel,
  effortIdForValue,
  LEGACY_EFFORTS,
  parseCurrentEffort,
  parseEffortChoices,
  parseModelOptions,
} from "./modelOption";

describe("parseModelOptions", () => {
  it("接受 Tauri 侧真实的 snake_case 形状", () => {
    const out = parseModelOptions([
      { id: "glm-coding", name: "GLM Coding Plan", endpoint_label: "open.bigmodel.cn" },
    ]);
    expect(out).toEqual([
      {
        id: "glm-coding",
        name: "GLM Coding Plan",
        endpointLabel: "open.bigmodel.cn",
        supportsEffort: false,
        effortOptions: [],
        defaultEffort: null,
      },
    ]);
  });

  it("name 缺失回退为 id；endpoint 缺失为空串（显示层只出 name）", () => {
    const out = parseModelOptions([{ id: "solo" }]);
    expect(out[0].name).toBe("solo");
    expect(out[0].endpointLabel).toBe("");
  });

  it("坏形状条目被丢弃，非数组当作空", () => {
    expect(parseModelOptions([{ name: "no-id" }, null, { id: "ok" }])).toHaveLength(1);
    expect(parseModelOptions("boom")).toEqual([]);
    expect(parseModelOptions(undefined)).toEqual([]);
  });

  it("C2：解析强度能力位（supports_effort / effort_options / default_effort）", () => {
    const out = parseModelOptions([
      {
        id: "glm-5.2",
        supports_effort: true,
        effort_options: [{ id: "low" }, { id: "high", label: "High" }],
        default_effort: "high",
      },
    ]);
    expect(out[0].supportsEffort).toBe(true);
    expect(out[0].effortOptions).toEqual([
      { id: "low", value: "low", label: "low" },
      { id: "high", value: "high", label: "High" },
    ]);
    expect(out[0].defaultEffort).toBe("high");
  });

  it("C2：能力位缺席 = 不支持（unknown ≠ advertised）", () => {
    const out = parseModelOptions([{ id: "plain" }]);
    expect(out[0].supportsEffort).toBe(false);
    expect(out[0].effortOptions).toEqual([]);
    expect(out[0].defaultEffort).toBeNull();
  });
});

describe("C2 effort parsing", () => {
  it("parseEffortChoices 收窄 StartResult.effort_options", () => {
    expect(parseEffortChoices([{ id: "low" }, { id: "deep", value: "xhigh", label: "Deep" }, "junk"])).toEqual([
      { id: "low", value: "low", label: "low" },
      { id: "deep", value: "xhigh", label: "Deep" },
    ]);
    expect(parseEffortChoices(undefined)).toEqual([]);
  });

  it("parseCurrentEffort 只认非空字符串", () => {
    expect(parseCurrentEffort("high")).toBe("high");
    expect(parseCurrentEffort("")).toBeNull();
    expect(parseCurrentEffort(null)).toBeNull();
    expect(parseCurrentEffort(42)).toBeNull();
  });

  it("effortStateForModel：不支持 → 空菜单（选择器不渲染）", () => {
    expect(effortStateForModel(undefined)).toEqual({ options: [], current: null });
    const plain = parseModelOptions([{ id: "plain" }])[0];
    expect(effortStateForModel(plain)).toEqual({ options: [], current: null });
  });

  it("effortStateForModel：catalog 菜单优先；空菜单回落 legacy 五档", () => {
    const withMenu = parseModelOptions([
      {
        id: "a",
        supports_effort: true,
        effort_options: [{ id: "deep", value: "xhigh", label: "Deep" }],
        default_effort: "xhigh",
      },
    ])[0];
    expect(effortStateForModel(withMenu)).toEqual({
      options: [{ id: "deep", value: "xhigh", label: "Deep" }],
      current: "deep",
    });
    const noMenu = parseModelOptions([{ id: "b", supports_effort: true }])[0];
    const st = effortStateForModel(noMenu);
    expect(st.options).toEqual(LEGACY_EFFORTS);
    expect(st.current).toBeNull();
  });

  it("自定义菜单 id 与 canonical value 分离，广播可映射回选择项", () => {
    const options = parseEffortChoices([{ id: "deep", value: "xhigh", label: "Deep" }]);
    expect(effortIdForValue(options, "xhigh")).toBe("deep");
    expect(effortIdForValue(options, "deep")).toBeNull();
  });
});
