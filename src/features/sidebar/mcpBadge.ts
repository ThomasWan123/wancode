/** Sidebar MCP badge: count servers loaded in this session, never configured-only. */
export function liveMcpBadgeCount(
  mcpLive: { session?: { enabled?: boolean } }[] | null | undefined,
): number {
  if (!mcpLive?.length) return 0;
  return mcpLive.filter((s) => s.session?.enabled !== false).length;
}
