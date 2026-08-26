export class WorkSnapshotIdentityError extends Error {
  constructor() {
    super("Work snapshot identity is not ready");
    this.name = "WorkSnapshotIdentityError";
  }
}

export async function snapshotWorkSourcesForSend(opts: {
  surface: "chat" | "code" | "work";
  workspaceId: string;
  sourcePaths: string[];
  snapshot: (workspaceId: string, sourcePaths: string[]) => Promise<void>;
}): Promise<void> {
  if (opts.surface !== "work") return;
  if (!opts.workspaceId) throw new WorkSnapshotIdentityError();
  await opts.snapshot(opts.workspaceId, opts.sourcePaths);
}
