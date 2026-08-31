/**
 * Observe the command that owns a primary model turn.
 *
 * Tauri events are useful for live updates, but they are not a safe completion
 * primitive: a renderer that subscribes late can miss `agent://turn-end` even
 * though the invoke itself resolves.  The command response is authoritative and
 * always settles the UI as a second, idempotent path.
 */
export function observePrimaryTurn(
  request: Promise<unknown>,
  onError: (error: unknown) => void,
  onSettled: () => void,
): void {
  request.catch(onError).finally(onSettled);
}
