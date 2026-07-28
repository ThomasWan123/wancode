/* v0.18.7-B：主下拉的结构化选项。value 永远是 catalog key；显示层给
   name + 脱敏端点 host——歧义选择器已经证明，同名模型只有端点能区分。 */
export type ModelOption = {
  id: string;
  name: string;
  endpointLabel: string;
};

/** Tauri 侧是 snake_case（serde 默认），边界处收窄并统一命名。 */
export function parseModelOptions(raw: unknown): ModelOption[] {
  if (!Array.isArray(raw)) return [];
  return raw
    .filter((o: any) => o && typeof o.id === "string")
    .map((o: any) => ({
      id: o.id,
      name: typeof o.name === "string" && o.name ? o.name : o.id,
      endpointLabel: typeof o.endpoint_label === "string" ? o.endpoint_label : "",
    }));
}
