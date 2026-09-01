/**
 * Observe the command that owns a primary model turn.
 *
 * Tauri events are useful for live updates, but they are not a safe completion
 * primitive: a renderer that subscribes late can miss `agent://turn-end` even
 * though the invoke itself resolves.  The command response is authoritative and
 * settles the UI as a second path while this request still owns the current
 * turn generation. A stale command must never settle its successor.
 */
export function observePrimaryTurn(
  request: Promise<unknown>,
  isCurrent: () => boolean,
  onError: (error: unknown) => void,
  onSettled: () => void,
): void {
  request
    .catch((error) => {
      if (isCurrent()) onError(error);
    })
    .finally(() => {
      if (isCurrent()) onSettled();
    });
}
