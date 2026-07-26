/* v0.18.6 步12：阻塞载荷的解析必须 fail-closed。
   Codex 第二十二轮指出：上一版未知 kind 直接返回 null，前端因此认为"没有
   阻塞"并放行发送，而引擎那边照旧挂着——正是刚修完的空 EndTurn 失配，
   从解析器这扇门重新进来了一次。 */
import { describe, expect, it } from "vitest";
import { parseModelBlock } from "./modelBlock";

describe("parseModelBlock", () => {
  it("解析歧义阻塞并保留候选", () => {
    const b = parseModelBlock({
      kind: "ambiguous_model_id",
      requested: "glm-4.6",
      candidates: [
        { id: "glm-open", name: "开放平台", endpointLabel: "open.bigmodel.cn", selectable: true },
        { id: "glm-coding", name: "Coding Plan", endpointLabel: "open.bigmodel.cn", selectable: true },
      ],
    });
    expect(b?.kind).toBe("ambiguous_model_id");
    expect(b && "candidates" in b && b.candidates).toHaveLength(2);
    expect(b && "candidates" in b && b.candidates[0].endpointLabel).toBe("open.bigmodel.cn");
  });

  it("解析不可用阻塞", () => {
    const b = parseModelBlock({ kind: "model_unavailable", requested: "gone-model" });
    expect(b).toEqual({ kind: "model_unavailable", requested: "gone-model" });
  });

  it("未知 kind 必须可见地阻塞，绝不静默放行", () => {
    const b = parseModelBlock({ kind: "some_future_reason", requested: "x" });
    expect(b).not.toBeNull();
    expect(b?.kind).toBe("unknown");
  });

  it("候选损坏时不给一个空选择器，而是当作读不懂", () => {
    const b = parseModelBlock({
      kind: "ambiguous_model_id",
      requested: "glm-4.6",
      // endpointLabel 缺失——正是 snake_case 那次事故的形状
      candidates: [{ id: "a", name: "A", selectable: true }],
    });
    expect(b?.kind).toBe("unknown");
  });

  it("部分候选损坏时整份作废，绝不只显示剩下的那些", () => {
    const b = parseModelBlock({
      kind: "ambiguous_model_id",
      requested: "glm-4.6",
      candidates: [
        { id: "glm-open", name: "开放平台", endpointLabel: "open.bigmodel.cn", selectable: true },
        // 这一条损坏。若只把它 filter 掉，用户会以为只有开放平台可选，
        // 从而把它固化——而这条损坏的才可能是原会话真正用的端点。
        { id: "glm-coding", name: "Coding Plan", selectable: true },
      ],
    });
    expect(b?.kind).toBe("unknown");
  });

  it("没有阻塞时才返回 null", () => {
    expect(parseModelBlock(null)).toBeNull();
    expect(parseModelBlock(undefined)).toBeNull();
  });

  it("非对象载荷同样阻塞", () => {
    expect(parseModelBlock("boom")?.kind).toBe("unknown");
  });
});
