/* v0.13 拆分：Git 面板（步 A：纯 JSX 搬移，依赖 props 透传）。
   红线提醒：worktree apply 一律 merge 模式；所有 git 操作走显式 gitRoot 通道（#83）。 */
import { invoke } from "@tauri-apps/api/core";
import { IconGitBranch } from "../../icons";

const baseName = (p: string) => p.split(/[\/]/).filter(Boolean).pop() ?? p;

export function GitPanel(props: Record<string, any>) {
  const { applyWorktree, changeLetter, commitMsg, createPr, prBusy, forkIntoWorktree, gitBranches, gitInfo, gitOp, refreshGit, removeWorktree, sendText, setCommitMsg, setError, setGitBranches, setItems, setShowGit, showGit, worktrees, wtBusy, wtMsg, t } = props;
  return (
    <>
      {showGit && (
        <div className="modal-mask" onClick={() => setShowGit(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title panel-title"><IconGitBranch size={16} /> {t.git}</div>
            {gitInfo?.isRepo === false || !gitInfo ? (
              <div className="modal-body">{t.gitNotRepo}</div>
            ) : (
              <>
                <div className="git-head">
                  <span className="git-branch-name">
                    <IconGitBranch size={13} /> {gitInfo.branch || "-"}
                  </span>
                  {(gitInfo.ahead ?? 0) > 0 && <span className="git-track">↑{gitInfo.ahead}</span>}
                  {(gitInfo.behind ?? 0) > 0 && <span className="git-track">↓{gitInfo.behind}</span>}
                  <select
                    className="git-branch-pick"
                    value=""
                    onChange={(e) => {
                      const b = e.currentTarget.value;
                      if (b) gitOp("git_checkout", { branch: b });
                    }}
                    onMouseDown={async () => {
                      if (gitBranches.length) return;
                      const r = await invoke<any>("git_branches").catch(() => null);
                      const list = r?.branches ?? r?.data?.branches ?? r?.result?.branches ?? [];
                      setGitBranches(
                        list.map((b: any) => (typeof b === "string" ? b : (b?.name ?? ""))).filter(Boolean),
                      );
                    }}
                  >
                    <option value="">{t.gitSwitchBranch}</option>
                    {gitBranches.map((b: any) => (
                      <option key={b} value={b}>
                        {b}
                      </option>
                    ))}
                  </select>
                </div>

                {(gitInfo.staged?.length ?? 0) === 0 && (gitInfo.unstaged?.length ?? 0) === 0 && (
                  <div className="sidebar-empty">{t.gitClean}</div>
                )}

                {(gitInfo.staged?.length ?? 0) > 0 && (
                  <div className="git-group">
                    <div className="git-group-head">
                      <span>{t.gitStaged(gitInfo.staged.length)}</span>
                      <button className="git-mini" onClick={() => gitOp("git_unstage", { paths: null })}>
                        {t.gitUnstageAll}
                      </button>
                    </div>
                    {gitInfo.staged.map((f: any) => (
                      <div key={f.path} className="git-row">
                        <span className="git-type">{changeLetter(f.type)}</span>
                        <span className="git-path">{f.path}</span>
                        <span className="git-stat">
                          <span className="add">+{f.additions ?? 0}</span>{" "}
                          <span className="del">-{f.deletions ?? 0}</span>
                        </span>
                        <button
                          className="git-mini"
                          title={t.gitUnstage}
                          onClick={() => gitOp("git_unstage", { paths: [f.path] })}
                        >
                          −
                        </button>
                      </div>
                    ))}
                  </div>
                )}

                {(gitInfo.unstaged?.length ?? 0) > 0 && (
                  <div className="git-group">
                    <div className="git-group-head">
                      <span>{t.gitChanges(gitInfo.unstaged.length)}</span>
                      <button className="git-mini" onClick={() => gitOp("git_stage", { paths: null })}>
                        {t.gitStageAll}
                      </button>
                    </div>
                    {gitInfo.unstaged.map((f: any) => (
                      <div key={f.path} className="git-row">
                        <span className="git-type">{changeLetter(f.type)}</span>
                        <span className="git-path">{f.path}</span>
                        <span className="git-stat">
                          <span className="add">+{f.additions ?? 0}</span>{" "}
                          <span className="del">-{f.deletions ?? 0}</span>
                        </span>
                        <button
                          className="git-mini"
                          title={t.gitStage}
                          onClick={() => gitOp("git_stage", { paths: [f.path] })}
                        >
                          +
                        </button>
                        <button
                          className="git-mini danger"
                          title={t.gitDiscard}
                          onClick={() => {
                            if (window.confirm(t.gitDiscardConfirm(f.path)))
                              gitOp("git_discard", { paths: [f.path] });
                          }}
                        >
                          ↺
                        </button>
                      </div>
                    ))}
                  </div>
                )}

                {/* worktree：把当前会话在独立工作树里再开一份，两边互不干扰
                    文件；做完要么合回主目录，要么整个丢掉。 */}
                <div className="git-group">
                  <div className="git-group-head">
                    <span>{t.wtSection}</span>
                    <button
                      className="git-mini"
                      disabled={wtBusy}
                      onClick={forkIntoWorktree}
                      title={t.wtNewHint}
                    >
                      {wtBusy ? t.wtWorking : t.wtNew}
                    </button>
                  </div>
                  {wtMsg && (
                    <div className={`wt-msg ${wtMsg.bad ? "bad" : "ok"}`}>{wtMsg.text}</div>
                  )}
                  {worktrees.length === 0 && <div className="sidebar-empty">{t.wtEmpty}</div>}
                  {worktrees.map((w: any) => (
                    <div key={w.path} className="git-row">
                      <span className="git-path" title={w.path}>
                        {baseName(w.path)}
                      </span>
                      {w.branch && <span className="git-track">{w.branch}</span>}
                      <button
                        className="git-mini"
                        disabled={wtBusy}
                        title={t.wtApplyHint}
                        onClick={() => applyWorktree(w.path)}
                      >
                        {t.wtApply}
                      </button>
                      <button
                        className="git-mini danger"
                        disabled={wtBusy}
                        onClick={() => {
                          if (window.confirm(t.wtRemoveConfirm(baseName(w.path))))
                            removeWorktree(w.path);
                        }}
                      >
                        {t.wtRemove}
                      </button>
                    </div>
                  ))}
                </div>

                <input
                  className="git-msg"
                  placeholder={t.gitMsgPlaceholder}
                  value={commitMsg}
                  onChange={(e) => setCommitMsg(e.currentTarget.value)}
                />
                <div className="git-actions">
                  <button
                    disabled={!commitMsg.trim() || (gitInfo.staged?.length ?? 0) === 0}
                    onClick={async () => {
                      await gitOp("git_commit", { message: commitMsg.trim() });
                      setCommitMsg("");
                    }}
                  >
                    {t.gitDoCommit}
                  </button>
                  <button
                    className="ghost"
                    disabled={(gitInfo.staged?.length ?? 0) + (gitInfo.unstaged?.length ?? 0) === 0}
                    title={t.gitStashHint}
                    onClick={async () => {
                      try {
                        await invoke("git_stash", { message: null });
                        setItems((prev: any[]) => [...prev, { kind: "note", text: t.gitStashed }]);
                        refreshGit();
                      } catch (e) {
                        setError(String(e));
                      }
                    }}
                  >
                    {t.gitStash}
                  </button>
                  <button
                    className="ghost"
                    disabled={prBusy || !gitInfo.branch || gitInfo.branch === "main" || gitInfo.branch === "master"}
                    title={t.gitPrHint}
                    onClick={() => createPr()}
                  >
                    {prBusy ? t.gitPrBusy : t.gitPr}
                  </button>
                  <button
                    className="ghost"
                    disabled={!gitInfo.files?.length}
                    onClick={() => {
                      setShowGit(false);
                      sendText(
                        "Review the git diff of my uncommitted changes, then create a well-formed git commit with a clear message.",
                      );
                    }}
                  >
                    {t.gitCommit}
                  </button>
                  <button
                    className="ghost"
                    disabled={!gitInfo.files?.length}
                    onClick={() => {
                      setShowGit(false);
                      sendText("Review my uncommitted changes for bugs and summarize what changed.");
                    }}
                  >
                    {t.gitReview}
                  </button>
                </div>
              </>
            )}
            <div className="modal-footer">
              <span />
              <button onClick={() => setShowGit(false)}>{t.close}</button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
