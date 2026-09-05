import { describe, expect, it } from "vitest";
import { engineDeadMessage, engineEventTargetsSession } from "./engineDead";

const LABEL = "会话引擎已退出";

describe("engineDeadMessage", () => {
  it("translates raw ENGINE_DEAD errors regardless of confirmation flag", () => {
    // 在飞调用：后端 map_acp_send_error 已带上结构化前缀，自身即充分信号。
    expect(
      engineDeadMessage(
        "ENGINE_DEAD: unable to send 'ext_method' request, channel closed: { \"xaiAcpChannelFailure\": \"send_failed\" }",
        false,
        LABEL,
      ),
    ).toBe(LABEL);
  });

  it("translates SESSION_NOT_STARTED only after the backend confirmed engine death", () => {
    expect(
      engineDeadMessage("SESSION_NOT_STARTED: 会话未启动", true, LABEL),
    ).toBe(LABEL);
    // 正常态的未启动会话不能被误翻成引擎退出。
    expect(
      engineDeadMessage("SESSION_NOT_STARTED: 会话未启动", false, LABEL),
    ).toBeNull();
    // Localized words are not a protocol discriminator.
    expect(engineDeadMessage("会话未启动", true, LABEL)).toBeNull();
  });

  it("passes unrelated errors through untouched", () => {
    // 负例：普通错误既不带前缀也不属于死亡下游，必须原样返回 null。
    expect(engineDeadMessage("当前工作区不是 git 仓库", true, LABEL)).toBeNull();
    expect(engineDeadMessage("worktree: some hub error", false, LABEL)).toBeNull();
    expect(engineDeadMessage("diagnostic mentions ENGINE_DEAD without a prefix", false, LABEL)).toBeNull();
    expect(
      engineDeadMessage("diagnostic mentions SESSION_NOT_STARTED: later", true, LABEL),
    ).toBeNull();
  });

  it("applies lifecycle events only to the exact live session", () => {
    expect(engineEventTargetsSession("session-a", "session-a")).toBe(true);
    expect(engineEventTargetsSession("session-a", "session-b")).toBe(false);
    expect(engineEventTargetsSession(undefined, "session-a")).toBe(false);
    expect(engineEventTargetsSession("", "session-a")).toBe(false);
  });
});
