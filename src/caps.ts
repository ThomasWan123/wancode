/* #127-3：能力/诊断的前端边界收窄（Tauri 侧 serde 是 snake_case）。
   与 modelOption.ts 同一纪律：wire 形状只在此处出现一次，组件消费
   收窄后的类型。 */

export type CapState = "supported" | "unsupported" | "unknown";
export type CapSource = "config" | "built_in" | "unknown";
export type Cap = { state: CapState; source: CapSource };
export type ModelCaps = {
  text: Cap;
  toolUse: Cap;
  visionInput: Cap;
  reasoning: Cap;
};
export type CapIssue = { catalogKey: string | null; field: string; kind: string };
export type ResolvedModelCaps = { caps: ModelCaps; issues: CapIssue[] };

export type FileIssue = { kind: "read_error" | "parse_error"; message: string };

export type ImageDecisionKind =
  | "allow_via_description"
  | "allow_native_vision"
  | "block_no_helper"
  | "block_helper_unavailable"
  | "block_helper_not_vision"
  | "warn_helper_unknown"
  | "block_main_not_vision"
  | "warn_main_unknown";

const UNKNOWN_CAP: Cap = { state: "unknown", source: "unknown" };

function parseCap(raw: any): Cap {
  const states: CapState[] = ["supported", "unsupported", "unknown"];
  const sources: CapSource[] = ["config", "built_in", "unknown"];
  if (!raw || !states.includes(raw.state) || !sources.includes(raw.source)) return UNKNOWN_CAP;
  return { state: raw.state, source: raw.source };
}

/** wire: { caps: { text, tool_use, vision_input, reasoning }, issues: [{catalog_key, field, kind}] } */
export function parseResolvedCaps(raw: unknown): ResolvedModelCaps {
  const r = raw as any;
  const c = r?.caps ?? {};
  return {
    caps: {
      text: parseCap(c.text),
      toolUse: parseCap(c.tool_use),
      visionInput: parseCap(c.vision_input),
      reasoning: parseCap(c.reasoning),
    },
    issues: Array.isArray(r?.issues)
      ? r.issues
          .filter((i: any) => i && typeof i.field === "string" && typeof i.kind === "string")
          .map((i: any) => ({
            catalogKey: typeof i.catalog_key === "string" ? i.catalog_key : null,
            field: i.field,
            kind: i.kind,
          }))
      : [],
  };
}

/** wire: { kind: "parse_error"|"read_error", message } —— StartResult.caps_config_issue。
    形状不符（含 camelCase 误写）一律返回 null：横幅只对真实诊断可见。 */
export function parseFileIssue(raw: unknown): FileIssue | null {
  const r = raw as any;
  if (!r || (r.kind !== "parse_error" && r.kind !== "read_error") || typeof r.message !== "string")
    return null;
  return { kind: r.kind, message: r.message };
}

/** wire: image_send_check 返回 { decision: { kind }, transcribe_on, helper_key } */
export function parseImageDecision(raw: unknown): ImageDecisionKind | null {
  const kinds: ImageDecisionKind[] = [
    "allow_via_description",
    "allow_native_vision",
    "block_no_helper",
    "block_helper_unavailable",
    "block_helper_not_vision",
    "warn_helper_unknown",
    "block_main_not_vision",
    "warn_main_unknown",
  ];
  const k = (raw as any)?.decision?.kind;
  return kinds.includes(k) ? k : null;
}

export type GateAction =
  | { action: "allow" }
  | { action: "block"; msg: "noHelper" | "helperUnavailable" | "helperNotVision" | "mainNotVision" | "unknownDecision" }
  | { action: "confirm"; msg: "helperUnknown" | "mainUnknown" };

/** 决策 → 前端动作。**fail-closed**：null/未知载荷一律 block，绝不放行。 */
export function imageGateAction(kind: ImageDecisionKind | null): GateAction {
  switch (kind) {
    case "allow_via_description":
    case "allow_native_vision":
      return { action: "allow" };
    case "block_no_helper":
      return { action: "block", msg: "noHelper" };
    case "block_helper_unavailable":
      return { action: "block", msg: "helperUnavailable" };
    case "block_helper_not_vision":
      return { action: "block", msg: "helperNotVision" };
    case "block_main_not_vision":
      return { action: "block", msg: "mainNotVision" };
    case "warn_helper_unknown":
      return { action: "confirm", msg: "helperUnknown" };
    case "warn_main_unknown":
      return { action: "confirm", msg: "mainUnknown" };
    default:
      return { action: "block", msg: "unknownDecision" };
  }
}
