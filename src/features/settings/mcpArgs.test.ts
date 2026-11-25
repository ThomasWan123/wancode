import { describe, expect, it } from "vitest";
import { parseMcpArgs } from "./mcpArgs";

/** 修复前的实现，原样保留——不带引号的输入必须和它逐字相同。 */
function legacySplit(text: string): string[] {
  return text.trim() ? text.trim().split(/\s+/) : [];
}

describe("parseMcpArgs", () => {
  it("splits unquoted input on whitespace without producing empty tokens", () => {
    expect(parseMcpArgs("  -y   @modelcontextprotocol/server-filesystem D:\\proj  ")).toEqual([
      "-y",
      "@modelcontextprotocol/server-filesystem",
      "D:\\proj",
    ]);
  });

  it("returns nothing for empty or whitespace-only input", () => {
    expect(parseMcpArgs("")).toEqual([]);
    expect(parseMcpArgs("   \t ")).toEqual([]);
  });

  it("keeps a double-quoted path with spaces as one argument", () => {
    expect(parseMcpArgs('-y server-filesystem "D:\\My Projects\\api"')).toEqual([
      "-y",
      "server-filesystem",
      "D:\\My Projects\\api",
    ]);
  });

  it("leaves backslashes inside quotes untouched", () => {
    expect(parseMcpArgs('"C:\\Program Files\\nodejs\\node.exe" server.js')).toEqual([
      "C:\\Program Files\\nodejs\\node.exe",
      "server.js",
    ]);
  });

  it("joins adjacent quoted and unquoted pieces into one argument", () => {
    expect(parseMcpArgs('prefix"two words"suffix')).toEqual(["prefixtwo wordssuffix"]);
  });

  it("takes an unterminated opening quote to the end of the line", () => {
    expect(parseMcpArgs('-y "C:\\Program Files\\x')).toEqual(["-y", "C:\\Program Files\\x"]);
  });

  it("treats an empty quoted string as one empty argument", () => {
    expect(parseMcpArgs('a "" b')).toEqual(["a", "", "b"]);
  });

  it("handles several quoted arguments on one line", () => {
    expect(parseMcpArgs('"D:\\a b" "D:\\c d"')).toEqual(["D:\\a b", "D:\\c d"]);
  });

  // 反向检查：修复只应该改变"带引号"这一种输入。对不带引号的输入，
  // 新实现必须和旧实现给出完全相同的结果，否则既有配置会被悄悄改写。
  it("is indistinguishable from the previous behaviour on unquoted input", () => {
    const unquoted = [
      "",
      "   ",
      "\t\n",
      "a",
      "a b c",
      "  a   b  ",
      "-y @modelcontextprotocol/server-filesystem D:\\proj",
      "--root=/d/my project",
      "路径 带中文",
      "a\tb",
      "a\nb",
      "single'quote",
      "trailing ",
      " leading",
    ];
    for (const text of unquoted) {
      expect(parseMcpArgs(text)).toEqual(legacySplit(text));
    }
  });
});
