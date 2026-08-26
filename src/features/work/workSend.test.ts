import { describe, expect, it } from "vitest";
import { snapshotWorkSourcesForSend, WorkSnapshotIdentityError } from "./workSend";

describe("snapshotWorkSourcesForSend", () => {
  it("uses the new Work identity and completes the snapshot before prompt continuation", async () => {
    const events: string[] = [];

    await snapshotWorkSourcesForSend({
      surface: "work",
      workspaceId: "work-new-session",
      sourcePaths: ["D:/docs/brief.pdf"],
      snapshot: async (workspaceId, sourcePaths) => {
        events.push(`snapshot:${workspaceId}:${sourcePaths[0]}`);
      },
    });
    events.push("prompt");

    expect(events).toEqual([
      "snapshot:work-new-session:D:/docs/brief.pdf",
      "prompt",
    ]);
  });

  it("fails closed instead of silently skipping a Work snapshot", async () => {
    await expect(
      snapshotWorkSourcesForSend({
        surface: "work",
        workspaceId: "",
        sourcePaths: [],
        snapshot: async () => {},
      }),
    ).rejects.toBeInstanceOf(WorkSnapshotIdentityError);
  });
});
