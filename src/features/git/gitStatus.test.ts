import { describe, expect, it } from "vitest";
import { gitErrorText, isNotGitRepoError, parseGitDiffsFiles } from "./gitStatus";

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

describe("parseGitDiffsFiles", () => {
  it("treats null data as not-a-repo, empty files as a clean repo", () => {
    expect(parseGitDiffsFiles({ result: { data: null } })).toBeNull();
    expect(parseGitDiffsFiles({ result: { data: { files: [] } } })).toEqual([]);
    expect(parseGitDiffsFiles({ error: "boom" })).toBeUndefined();
  });
});
