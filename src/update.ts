/* #121：更新流程。设计约束见 update.test.ts 头注——核心是三件事：
 * 下载进度可见、安装前先落标记再告知、下次启动对账成功/未完成。
 *
 * Windows 地面真相（tauri-plugin-updater 2.10.1）：install() 拉起 NSIS
 * （默认 Passive：/P /R，有进度条、装完自动重启应用）后立即
 * std::process::exit(0)，且不检查安装器是否真的启动成功。因此：
 *   - install() 之后的代码在 Windows 上永远不会执行；
 *   - "安装器没起来"与"安装成功"在退出瞬间不可区分——只能靠下次启动
 *     时的版本对账（checkPostUpdate）事后判定。 */

export const UPDATE_MARKER_KEY = "wancode-update-attempt";

type UpdateHandle = {
  version: string;
  download(onEvent?: (e: DownloadEvent) => void): Promise<void>;
  install(): Promise<void>;
};

type DownloadEvent =
  | { event: "Started"; data: { contentLength?: number } }
  | { event: "Progress"; data: { chunkLength: number } }
  | { event: "Finished" };

type StorageLike = {
  getItem(k: string): string | null;
  setItem(k: string, v: string): void;
  removeItem(k: string): void;
};

export type UpdateDeps = {
  check: () => Promise<UpdateHandle | null>;
  relaunch: () => Promise<void>;
  currentVersion: string;
  setMsg: (m: string) => void;
  storage: StorageLike;
  t: {
    checkingUpdate: string;
    upToDate: (v: string) => string;
    updateAvailable: (v: string) => string;
    updateDownloading: (v: string, pct: number) => string;
    updateInstalling: (v: string) => string;
    updateFailed: string;
    updateSucceeded: (v: string) => string;
    updateIncomplete: (v: string) => string;
  };
};

export async function runUpdateFlow(deps: UpdateDeps): Promise<void> {
  const { setMsg, t } = deps;
  setMsg(t.checkingUpdate);
  try {
    const update = await deps.check();
    if (!update) {
      setMsg(t.upToDate(deps.currentVersion));
      return;
    }
    setMsg(t.updateAvailable(update.version));

    let total = 0;
    let received = 0;
    await update.download((e) => {
      if (e.event === "Started") {
        total = e.data.contentLength ?? 0;
      } else if (e.event === "Progress") {
        received += e.data.chunkLength;
        const pct = total > 0 ? Math.min(100, Math.round((received / total) * 100)) : 0;
        setMsg(t.updateDownloading(update.version, pct));
      }
    });

    // 标记必须写在 install 之前：Windows 上 install 一调用进程就没了。
    deps.storage.setItem(
      UPDATE_MARKER_KEY,
      JSON.stringify({ from: deps.currentVersion, to: update.version }),
    );
    // 明确告知将要发生什么——应用关闭不再是"无声消失"。
    setMsg(t.updateInstalling(update.version));
    await update.install();
    // Windows 到不了这里（进程已由插件退出，安装器 /R 负责重启）；
    // macOS/Linux 走这条完成重启。
    await deps.relaunch();
  } catch (e) {
    // 失败不能留下"升级中"的假象——标记只属于真正进入安装的那一次。
    deps.storage.removeItem(UPDATE_MARKER_KEY);
    setMsg(deps.t.updateFailed + String(e));
  }
}

/** 启动时对账上一次升级：成功 / 未完成 / 无事发生。返回给用户看的话。 */
export function checkPostUpdate(deps: UpdateDeps): string | null {
  const raw = deps.storage.getItem(UPDATE_MARKER_KEY);
  if (!raw) return null;
  // 一次性标记：无论结论如何都清掉，避免每次启动重复弹；提示里已含重试引导。
  deps.storage.removeItem(UPDATE_MARKER_KEY);
  let marker: { from?: string; to?: string };
  try {
    marker = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!marker.to) return null;
  if (deps.currentVersion === marker.to) return deps.t.updateSucceeded(marker.to);
  // 版本没动——安装器没跑成（被拦/被关/崩了）。这正是 #121 里用户
  // 无从得知的那件事，现在开口说出来。
  return deps.t.updateIncomplete(marker.to);
}
