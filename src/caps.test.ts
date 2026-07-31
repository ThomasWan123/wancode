import { describe, expect, it } from "vitest";
import {
  imageGateAction,
  parseFileIssue,
  parseImageDecision,
  parseResolvedCaps,
} from "./caps";

describe("caps 边界收窄（真实 snake_case wire 形状）", () => {
  it("按 snake_case 解析 ResolvedModelCaps", () => {
    const wire = {
      caps: {
        text: { state: "supported", source: "built_in" },
        tool_use: { state: "unknown", source: "unknown" },
        vision_input: { state: "unsupported", source: "config" },
        reasoning: { state: "supported", source: "config" },
      },
      issues: [{ catalog_key: "m1", field: "vision_input", kind: "wrong_type" }],
    };
    const r = parseResolvedCaps(wire);
    expect(r.caps.visionInput).toEqual({ state: "unsupported", source: "config" });
    expect(r.caps.toolUse.state).toBe("unknown");
    expect(r.issues).toEqual([{ catalogKey: "m1", field: "vision_input", kind: "wrong_type" }]);
  });

  it("形状不符逐项回落 unknown，不抛错", () => {
    const r = parseResolvedCaps({ caps: { text: { state: "yes" } }, issues: "nope" });
    expect(r.caps.text.state).toBe("unknown");
    expect(r.issues).toEqual([]);
  });

  it("caps_config_issue：真实 snake_case kind/message 才可见", () => {
    expect(parseFileIssue({ kind: "parse_error", message: "config.toml: bad" })).toEqual({
      kind: "parse_error",
      message: "config.toml: bad",
    });
    expect(parseFileIssue({ kind: "read_error", message: "io" })?.kind).toBe("read_error");
    // 边界：camelCase 误写 / 未知 kind / 缺 message → 横幅不可见（null）
    expect(parseFileIssue({ kind: "parseError", message: "x" })).toBeNull();
    expect(parseFileIssue({ kind: "parse_error" })).toBeNull();
    expect(parseFileIssue(null)).toBeNull();
    expect(parseFileIssue(undefined)).toBeNull();
  });

  it("image_send_check 决策 kind 白名单", () => {
    expect(parseImageDecision({ decision: { kind: "allow_via_description" } })).toBe(
      "allow_via_description",
    );
    expect(parseImageDecision({ decision: { kind: "block_no_helper" } })).toBe("block_no_helper");
    expect(parseImageDecision({ decision: { kind: "surprise" } })).toBeNull();
    expect(parseImageDecision({})).toBeNull();
  });

  it("门控动作映射：Block 与未知载荷零发送（fail-closed），Warn 走确认", () => {
    // Block 四型 → block
    for (const k of [
      "block_no_helper",
      "block_helper_unavailable",
      "block_helper_not_vision",
      "block_main_not_vision",
    ] as const) {
      expect(imageGateAction(k).action).toBe("block");
    }
    // 未知/null 载荷 → block（绝不放行）
    expect(imageGateAction(null)).toEqual({ action: "block", msg: "unknownDecision" });
    // Warn → confirm（取消即零发送由 App 分支保证：confirm=false 直接 return）
    expect(imageGateAction("warn_helper_unknown")).toEqual({
      action: "confirm",
      msg: "helperUnknown",
    });
    expect(imageGateAction("warn_main_unknown").action).toBe("confirm");
    // Allow 两型
    expect(imageGateAction("allow_via_description").action).toBe("allow");
    expect(imageGateAction("allow_native_vision").action).toBe("allow");
  });
});
