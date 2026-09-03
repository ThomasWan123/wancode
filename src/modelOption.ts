/* v0.18.7-B：主下拉的结构化选项。value 永远是 catalog key；显示层给
   name + 脱敏端点 host——歧义选择器已经证明，同名模型只有端点能区分。 */
export type ModelOption = {
  id: string;
  name: string;
  endpointLabel: string;
  /** C2：引擎能力位——该模型是否支持推理强度（unknown ≠ advertised）。 */
  supportsEffort: boolean;
  /** C2：catalog 声明的强度菜单；空数组 = 引擎回落 legacy 五档。 */
  effortOptions: EffortChoice[];
  /** C2：config.toml 里该模型的默认强度档。 */
  defaultEffort: string | null;
};

/** C2：强度菜单的一项（引擎 sessionConfig 的 mode 条目 / catalog 条目）。 */
export type EffortChoice = { id: string; value: string; label: string };

/**
 * Cold-start compatibility default for installations whose config predates
 * newer presets. This id must remain resolvable without forcing an existing
 * user to rerun quick setup; new presets are selected after the catalog loads.
 */
export const SAFE_INITIAL_MODEL_ID = "glm-5.2";

/** Model picker fallback used only when both live model sources are absent. */
export const FALLBACK_MODEL_IDS = [
  "glm-5.2",
  "glm-5-turbo",
  "glm-4-flash",
  "deepseek-chat",
  "deepseek-reasoner",
] as const;

/** Tauri 侧是 snake_case（serde 默认），边界处收窄并统一命名。 */
export function parseModelOptions(raw: unknown): ModelOption[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((o: any) => o && typeof o.id === "string")
    .map((o: any) => ({
      id: o.id,
      name: typeof o.name === "string" && o.name ? o.name : o.id,
      endpointLabel: typeof o.endpoint_label === "string" ? o.endpoint_label : "",
      supportsEffort: o.supports_effort === true,
      effortOptions: parseEffortChoices(o.effort_options),
      defaultEffort: typeof o.default_effort === "string" ? o.default_effort : null,
    }));
}

/** StartResult.effort_options（Rust 已解析）的边界收窄。 */
export function parseEffortChoices(raw: unknown): EffortChoice[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((o: any) => o && typeof o.id === "string")
    .map((o: any) => ({
      id: o.id,
      value: typeof o.value === "string" && o.value ? o.value : o.id,
      label: typeof o.label === "string" && o.label ? o.label : o.id,
    }));
}

/** StartResult.current_effort 的边界收窄。 */
export function parseCurrentEffort(raw: unknown): string | null {
  return typeof raw === "string" && raw ? raw : null;
}

/* 引擎 legacy 五档兜底（xai-grok-shell session_config.rs 的
   SELECTABLE_REASONING_EFFORTS）：模型支持强度但 catalog 没给菜单时，
   引擎在 sessionConfig 里下发这五档。热切换模型后 sessionConfig 不重来，
   前端用同一份兜底推导新模型的菜单；引擎 set_model 侧对不支持的档会
   忽略并 warn，发错档不会写坏会话。 */
export const LEGACY_EFFORTS: EffortChoice[] = [
  { id: "minimal", value: "minimal", label: "Minimal" },
  { id: "low", value: "low", label: "Low" },
  { id: "medium", value: "medium", label: "Medium" },
  { id: "high", value: "high", label: "High" },
  { id: "xhigh", value: "xhigh", label: "X-High" },
];

/** 热切换模型后的强度选择器状态：能力位来自引擎 meta；菜单空则按引擎
    规则回落 legacy 五档；当前档先取该模型的配置默认（引擎事务完成后
    会以 ModelChanged 广播回校准）。不支持 → 空菜单（不显示选择器）。 */
export function effortStateForModel(opt: ModelOption | undefined): {
  options: EffortChoice[];
  current: string | null;
} {
  if (!opt || !opt.supportsEffort) return { options: [], current: null };
  return {
    options: opt.effortOptions.length ? opt.effortOptions : LEGACY_EFFORTS,
    current:
      (opt.effortOptions.length ? opt.effortOptions : LEGACY_EFFORTS)
        .find((choice) => choice.value === opt.defaultEffort)?.id ?? null,
  };
}

/** ModelChanged 广播携带 canonical value；选择框保存菜单 id。 */
export function effortIdForValue(options: EffortChoice[], value: unknown): string | null {
  if (typeof value !== "string" || !value) return null;
  return options.find((choice) => choice.value === value)?.id ?? null;
}
