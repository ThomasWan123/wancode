/* v0.18.6：会话为什么现在不能发送。
   引擎在 LoadSessionResponse.meta["x.ai/modelBlock"] 里给出，Tauri 经
   StartResult.model_block 透传。

   两条设计原则，都是被同一个失败模式教出来的——"前端以为没事、引擎其实
   挂着"，用户点发送收到一个空 EndTurn，什么解释都没有：

   1. 解析 fail-closed。载荷不认识不代表没有阻塞，只代表我们读不懂它。
      读不懂就当作阻塞并说明情况，绝不放行。
   2. 穷举而非条件判断。下面的 assertNever 让新增 kind 在编译期就炸，
      而不是等到某个用户撞上没有 UI 分支的那一类。 */

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
};

/** 引擎说这个会话被挂起了，但载荷的形状我们读不懂（版本不匹配、字段损坏）。 */
export type UnknownBlock = {
  kind: "unknown";
  /** 原始 kind 字符串，用于显示和排查。 */
  raw: string;
};

export type ModelBlock = AmbiguousBlock | UnavailableBlock | UnknownBlock;

/** 编译期穷举保证：新增 ModelBlock 成员而没处理，这里会报类型错误。 */
export function assertNever(x: never): never {
  throw new Error(`unhandled model block: ${JSON.stringify(x)}`);
}

function isCandidate(c: unknown): c is AmbiguousCandidate {
  const v = c as any;
  return (
    !!v &&
    typeof v === "object" &&
    typeof v.id === "string" &&
    typeof v.name === "string" &&
    typeof v.endpointLabel === "string" &&
    typeof v.selectable === "boolean"
  );
}

/**
 * 把 Tauri 边界过来的未知值收窄成 ModelBlock。
 *
 * `null` 只有一个含义：引擎没有报告任何阻塞。其余一切——未知 kind、字段
 * 缺失、候选损坏——都落到 `unknown`，因为"我们读不懂"和"没有阻塞"是两件
 * 完全不同的事，把前者当后者处理正是会放行一次注定失败的发送。
 */
export function parseModelBlock(raw: unknown): ModelBlock | null {
  if (raw === null || raw === undefined) return null;
  const b = raw as any;
  if (typeof b !== "object") return { kind: "unknown", raw: String(raw) };

  const rawKind = typeof b.kind === "string" ? b.kind : "";
  const requested = typeof b.requested === "string" ? b.requested : "";

  if (rawKind === "ambiguous_model_id") {
    // 全有或全无。filter 掉坏的那些看起来更"健壮"，实际是最危险的做法：
    // A 合法、B 损坏时用户只看到 A，会以为只有这一个选择并把它固化下来，
    // 而 B 才可能是原会话真正用的那个端点——他连"少了一个"都无从知道。
    // 一条读不懂，整份载荷就不可信，宁可说"读不懂"也不给残缺的选项。
    const raw = Array.isArray(b.candidates) ? b.candidates : null;
    if (!raw || raw.length === 0 || !raw.every(isCandidate)) {
      return { kind: "unknown", raw: rawKind };
    }
    return { kind: "ambiguous_model_id", requested, candidates: raw };
  }
  if (rawKind === "model_unavailable") {
    return { kind: "model_unavailable", requested };
  }
  return { kind: "unknown", raw: rawKind };
}
