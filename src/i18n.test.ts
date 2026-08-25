import { describe, expect, it } from "vitest";

import { displaySessionTitle, loadLang, STRINGS } from "./i18n";
import { surfaceLabel } from "./surface";

describe("displaySessionTitle", () => {
  it.each([undefined, null, "", "(未命名会话)", "(untitled session)"])(
    "uses the active locale for an empty or legacy placeholder title (%s)",
    (title) => {
      expect(displaySessionTitle(title, "(untitled session)")).toBe("(untitled session)");
    },
  );

  it("preserves a real session title", () => {
    expect(displaySessionTitle("Fix login race", "(untitled session)")).toBe("Fix login race");
  });
});

describe("surface switcher i18n", () => {
  it("defaults to zh and uses 聊天 / 代码 / 工作", () => {
    localStorage.removeItem("wancode-lang");
    expect(loadLang()).toBe("zh");
    expect(STRINGS.zh.surfaceChat).toBe("聊天");
    expect(STRINGS.zh.surfaceCode).toBe("代码");
    expect(STRINGS.zh.surfaceWork).toBe("工作");
    expect(surfaceLabel("chat", STRINGS.zh)).toBe("聊天");
    expect(surfaceLabel("code", STRINGS.zh)).toBe("代码");
    expect(surfaceLabel("work", STRINGS.zh)).toBe("工作");
  });

  it("keeps English Chat / Code / Work", () => {
    expect(STRINGS.en.surfaceChat).toBe("Chat");
    expect(STRINGS.en.surfaceCode).toBe("Code");
    expect(STRINGS.en.surfaceWork).toBe("Work");
    expect(surfaceLabel("chat", STRINGS.en)).toBe("Chat");
    expect(surfaceLabel("code", STRINGS.en)).toBe("Code");
    expect(surfaceLabel("work", STRINGS.en)).toBe("Work");
  });

  it("shares one friendly empty-git string for Diff and Review", () => {
    expect(STRINGS.zh.gitNeedRepo).toBe("先打开一个 git 项目才能看改动 / 审查。");
    expect(STRINGS.en.gitNeedRepo).toBe("Open a git project to see changes and run Review.");
    expect(STRINGS.zh.mentionNoFiles).toMatch(/文件/);
  });
});
