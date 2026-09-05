/**
 * 引擎退出（engine-dead）后的错误翻译。
 *
 * 背景：引擎线程意外退出后，后端会摘掉 handle 并广播 `agent://engine-dead`；
 * 但摘除前在飞的调用回 `ENGINE_DEAD: …`（后端 map_acp_send_error），摘除后
 * 的后续调用统一回「会话未启动」。本模块把这两族错误翻成同一条用户可读
 * 文案，避免每个面板各自弹一条天书（实测用户看到的就是
 * `worktree: unable to send 'ext_method' request, channel closed: …`）。
 */

/**
 * 把一个错误串翻译成「引擎已退出」文案。
 *
 * @param raw       原始错误串（String(e)）
 * @param confirmedDead 后端已广播 engine-dead（此后「会话未启动」= 引擎死亡
 *                      的下游症状，而非从未启动；未确认时不做这种引申）
 * @param label     引擎退出文案（i18n 的 engineDead）
 * @returns 翻译结果；不属于引擎死亡两族错误时返回 null（调用方原样展示）
 */
export function engineDeadMessage(
  raw: string,
  confirmedDead: boolean,
  label: string,
): string | null {
  if (raw.trimStart().startsWith("ENGINE_DEAD:")) return label;
  if (confirmedDead && raw.includes("会话未启动")) return label;
  return null;
}

/**
 * Engine lifecycle events belong to exactly one session. Missing identity is
 * rejected rather than allowed to poison whichever session happens to be live.
 */
export function engineEventTargetsSession(
  eventSessionId: unknown,
  liveSessionId: string,
): boolean {
  return typeof eventSessionId === "string"
    && eventSessionId.length > 0
    && eventSessionId === liveSessionId;
}
