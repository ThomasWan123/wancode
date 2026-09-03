import { describe, expect, it } from "vitest";
import {
  gitErrorText,
  isCapabilityDeniedError,
  isNotGitRepoError,
  parseGitDiffsFiles,
} from "./gitStatus";

describe("isNotGitRepoError", () => {
  it("recognizes the engine reject and the i18n copy, ignoring case of git", () => {
    expect(isNotGitRepoError("当前工作区不是 git 仓库")).toBe(true);
    expect(isNotGitRepoError("当前工作区不是 Git 仓库")).toBe(true);
    expect(isNotGitRepoError("This workspace is not a Git repo")).toBe(true);
    expect(isNotGitRepoError({ message: "diff: 当前工作区不是 git 仓库" })).toBe(true);
    expect(isNotGitRepoError("network timeout")).toBe(false);
  });

  it("reads Tauri-style wrapped errors", () => {
    expect(gitErrorText({ error: "当前工作区不是 git 仓库" })).toContain("git");
  });
});

describe("isCapabilityDeniedError", () => {
  it("matches the live lease denials the backend emits for read-class ext calls", () => {
    // 与 agent.rs ext_call 的拒绝文案逐字对齐（refreshWorktrees 的真实
    // 失败路径）。
    expect(
      isCapabilityDeniedError(
        "CAPABILITY_EXTENSION_BLOCKED: x.ai/git/worktree/list: tool is denied: read.",
      ),
    ).toBe(true);
    expect(isCapabilityDeniedError("CAPABILITY_PATH_BLOCKED: x.ai/fs/read_file: …")).toBe(true);
    expect(isCapabilityDeniedError({ error: "CAPABILITY_EXTENSION_BLOCKED: x.ai/git/status: tool is denied: read." })).toBe(true);
  });

  it("does not swallow unrelated failures", () => {
    expect(isCapabilityDeniedError("network timeout")).toBe(false);
    expect(isCapabilityDeniedError("APPROVAL_RECEIPT_STALE: #1")).toBe(false);
    // 租约失效是另一种故障（会话/租约错位），不是"层没有该能力"。
    expect(isCapabilityDeniedError("CAPABILITY_LEASE_INVALID: …")).toBe(false);
  });
});

describe("parseGitDiffsFiles", () => {
  it("treats null data as not-a-repo, empty files as a clean repo", () => {
    expect(parseGitDiffsFiles({ result: { data: null } })).toBeNull();
    expect(parseGitDiffsFiles({ result: { data: { files: [] } } })).toEqual([]);
    expect(parseGitDiffsFiles({ error: "boom" })).toBeUndefined();
  });
});
