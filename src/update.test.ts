/* #121（RED）：更新流程必须让用户在每个阶段都知道发生了什么。
 *
 * Dogfooding 实证（2026-07-28）：用户点更新 → 下载后无界面、不重启、
 * 版本仍 0.18.5。地面真相（tauri-plugin-updater 2.10.1 源码）：
 *   - Windows 上 install() 用 ShellExecuteW 拉起安装器，不检查结果，
 *     直接 std::process::exit(0)——安装器没起来时应用就这么无声消失；
 *   - downloadAndInstall 之后的 relaunch() 是死代码（进程已退出）；
 *   - 我们原来的 UI 没接下载进度回调，下载阶段全程盲盒。
 * 三个断口叠加 = "无法区分升级成功与什么都没发生"。
 *
 * 修法不依赖插件内部行为：下载进度可见 → 装前明确告知"将关闭并自动
 * 安装" → 装前落一个升级标记，下次启动对账（版本升了报成功；版本没变
 * 但标记在，报"上次升级未完成"）。 */
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  UPDATE_MARKER_KEY,
  checkPostUpdate,
  runUpdateFlow,
  type UpdateDeps,
} from "./update";
import { STRINGS } from "./i18n";

const t = STRINGS.zh;

function makeDeps(overrides: Partial<UpdateDeps> = {}) {
  const messages: string[] = [];
  const store = new Map<string, string>();
  const deps: UpdateDeps = {
    check: vi.fn(async () => null),
    relaunch: vi.fn(async () => {}),
    currentVersion: "0.18.5",
    setMsg: (m: string) => messages.push(m),
    storage: {
      getItem: (k) => store.get(k) ?? null,
      setItem: (k, v) => void store.set(k, v),
      removeItem: (k) => void store.delete(k),
    },
    t,
    ...overrides,
  };
  return { deps, messages, store };
}

const fakeUpdate = (impl: Partial<{ download: any; install: any }> = {}) => ({
  version: "0.18.6",
  download: vi.fn(async (onEvent?: (e: any) => void) => {
    onEvent?.({ event: "Started", data: { contentLength: 1000 } });
    onEvent?.({ event: "Progress", data: { chunkLength: 500 } });
    onEvent?.({ event: "Finished" });
  }),
  install: vi.fn(async () => {}),
  ...impl,
});

describe("runUpdateFlow", () => {
  it("已是最新时明确说出当前版本", async () => {
    const { deps, messages } = makeDeps();
    await runUpdateFlow(deps);
    expect(messages[messages.length - 1]).toBe(t.upToDate("0.18.5"));
  });

  it("下载阶段报告可见进度——不能再是盲盒", async () => {
    const upd = fakeUpdate();
    const { deps, messages } = makeDeps({ check: vi.fn(async () => upd as any) });
    await runUpdateFlow(deps);
    // 至少出现一条带百分比的进度消息（500/1000 = 50%）。
    expect(messages.some((m) => m.includes("50%"))).toBe(true);
  });

  it("安装前先落升级标记再告知用户——顺序不能反", async () => {
    const events: string[] = [];
    const upd = fakeUpdate({
      install: vi.fn(async () => void events.push("install")),
    });
    const { deps, store } = makeDeps({
      check: vi.fn(async () => upd as any),
      storage: {
        getItem: () => null,
        setItem: (k) => void events.push(`marker:${k}`),
        removeItem: () => {},
      },
    });
    await runUpdateFlow(deps);
    // 标记必须写在 install 之前：Windows 上 install 一调用进程就没了，
    // 之后的任何代码都不存在"稍后再写"。
    expect(events.indexOf(`marker:${UPDATE_MARKER_KEY}`)).toBeGreaterThanOrEqual(0);
    expect(events.indexOf(`marker:${UPDATE_MARKER_KEY}`)).toBeLessThan(events.indexOf("install"));
    void store;
  });

  it("安装前的提示写明：应用将关闭并自动安装、完成后自动重开", async () => {
    const upd = fakeUpdate();
    const { deps, messages } = makeDeps({ check: vi.fn(async () => upd as any) });
    await runUpdateFlow(deps);
    expect(messages.some((m) => m === t.updateInstalling("0.18.6"))).toBe(true);
  });

  it("检查/下载失败时报错并清掉标记——失败不能留下'升级中'假象", async () => {
    const upd = fakeUpdate({
      download: vi.fn(async () => {
        throw new Error("net down");
      }),
    });
    const { deps, messages, store } = makeDeps({ check: vi.fn(async () => upd as any) });
    await runUpdateFlow(deps);
    expect(messages[messages.length - 1]).toContain(t.updateFailed);
    expect(store.has(UPDATE_MARKER_KEY)).toBe(false);
  });
});

describe("checkPostUpdate（启动对账）", () => {
  beforeEach(() => vi.clearAllMocks());

  it("版本升上去了：报成功并清标记", () => {
    const { deps, store } = makeDeps({ currentVersion: "0.18.6" });
    store.set(UPDATE_MARKER_KEY, JSON.stringify({ from: "0.18.5", to: "0.18.6" }));
    const out = checkPostUpdate(deps);
    expect(out).toBe(t.updateSucceeded("0.18.6"));
    expect(store.has(UPDATE_MARKER_KEY)).toBe(false);
  });

  it("版本没变但标记还在：明确说'上次升级未完成'——这正是 #121 里用户无法得知的那件事", () => {
    const { deps, store } = makeDeps({ currentVersion: "0.18.5" });
    store.set(UPDATE_MARKER_KEY, JSON.stringify({ from: "0.18.5", to: "0.18.6" }));
    const out = checkPostUpdate(deps);
    expect(out).toBe(t.updateIncomplete("0.18.6"));
    // 标记保留？不——清掉，避免每次启动都弹；提示里已引导重试。
    expect(store.has(UPDATE_MARKER_KEY)).toBe(false);
  });

  it("无标记：什么都不说", () => {
    const { deps } = makeDeps();
    expect(checkPostUpdate(deps)).toBeNull();
  });

  it("标记损坏：静默清除，不炸启动", () => {
    const { deps, store } = makeDeps();
    store.set(UPDATE_MARKER_KEY, "{not json");
    expect(checkPostUpdate(deps)).toBeNull();
    expect(store.has(UPDATE_MARKER_KEY)).toBe(false);
  });
});
