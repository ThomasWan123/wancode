/* v0.18.6：会话为什么现在不能发送。
   引擎在 LoadSessionResponse.meta["x.ai/modelBlock"] 里给出，Tauri 经
   StartResult.model_block 透传。

   这里用可辨识联合而不是 any，是因为上一版就吃过亏：modelBlock 是 any 时，
   Composer 只处理了 ambiguous_model_id，编译器不会提醒 model_unavailable
   没有 UI 分支，于是那类阻塞掉进死区——发送按钮看着能点，点了被 App 静默
   吞掉，用户得不到任何解释。以后新增 kind 时，下面的 switch 会当场编译失败。 */

export type AmbiguousCandidate = {
  id: string;
  name: string;
  /** base_url 的 host，引擎侧已剥掉凭据与 query。同名模型靠它区分。 */
  endpointLabel: string;
  /** allowed_models 是否允许选它。false 仍然展示，只是不给点。 */
  selectable: boolean;
};

/** 同一个名字对应多个已配置条目，只有用户知道当初用的是哪个。 */
export type AmbiguousBlock = {
  kind: "ambiguous_model_id";
  requested: string;
  candidates: AmbiguousCandidate[];
};

/** 会话原来那个模型已经不在目录里了，没有候选可选。 */
export type UnavailableBlock = {
  kind: "model_unavailable";
  requested: string;
  candidates: [];
};

export type ModelBlock = AmbiguousBlock | UnavailableBlock;

/** Tauri 边界过来的是 unknown——收窄一次，形状不认识就当没有阻塞。 */
export function parseModelBlock(raw: unknown): ModelBlock | null {
  const b = raw as any;
  if (!b || typeof b !== "object") return null;
  if (b.kind === "ambiguous_model_id") {
    return {
      kind: "ambiguous_model_id",
      requested: String(b.requested ?? ""),
      candidates: Array.isArray(b.candidates) ? b.candidates : [],
    };
  }
  if (b.kind === "model_unavailable") {
    return { kind: "model_unavailable", requested: String(b.requested ?? ""), candidates: [] };
  }
  return null;
}
