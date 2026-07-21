/* v0.13 拆分：后台任务/子 Agent/定时任务面板。
   定时任务视图由通知流重建（created 幂等 upsert，fired 只更新时间）。 */
import { invoke } from "@tauri-apps/api/core";
import { IconTerminal } from "../../icons";

export function TasksPanel(props: Record<string, any>) {
  const { bgTasks, refreshTasks, schedTasks, setError, setShowTasks, showTasks, subagents, worktrees, openWorktree, t } = props;
  return (
    <>
      {showTasks && (
        <div className="modal-mask" onClick={() => setShowTasks(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <div className="modal-title panel-title">
              <IconTerminal size={16} /> {t.tasksTitle}
            </div>

            {bgTasks.length === 0 &&
              subagents.length === 0 &&
              (worktrees?.length ?? 0) === 0 &&
              Object.keys(schedTasks).length === 0 && (
                <div className="sidebar-empty">{t.tasksEmpty}</div>
              )}

            {(worktrees?.length ?? 0) > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksWorktrees(worktrees.length)}</span>
                </div>
                {worktrees.map((w: any) => (
                  <div key={w.path} className="git-row">
                    <span className="git-path" title={w.path}>
                      {(w.path.split(/[\/]/).filter(Boolean).pop() ?? w.path)}
                    </span>
                    {w.branch && <span className="git-track">{w.branch}</span>}
                    <button className="git-mini" onClick={() => openWorktree(w)}>
                      {w.sessionId ? t.wtOpenResume : t.wtOpenNew}
                    </button>
                  </div>
                ))}
              </div>
            )}

            {bgTasks.length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksBg(bgTasks.length)}</span>
                </div>
                {bgTasks.map((b: any) => {
                  // TaskSnapshot 是 snake_case
                  const id = b.task_id ?? b.taskId;
                  const cmd = b.display_command ?? b.command ?? "";
                  const done = b.completed === true;
                  return (
                    <div key={id} className="task-row">
                      <span className={`task-dot ${done ? "done" : "run"}`} />
                      <span className="task-cmd" title={cmd}>
                        {cmd}
                      </span>
                      {done ? (
                        <span className="task-exit">
                          {b.exit_code === 0 ? "exit 0" : `exit ${b.exit_code ?? "?"}`}
                        </span>
                      ) : (
                        <button
                          className="git-mini danger"
                          onClick={async () => {
                            await invoke("task_kill", { taskId: id }).catch((e) =>
                              setError(String(e)),
                            );
                            refreshTasks();
                          }}
                        >
                          {t.taskKill}
                        </button>
                      )}
                    </div>
                  );
                })}
              </div>
            )}

            {subagents.length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksSub(subagents.length)}</span>
                </div>
                {subagents.map((sa: any) => (
                  // SubagentLiveSnapshotDto 是 camelCase
                  <div key={sa.subagentId} className="task-row">
                    <span className="task-dot run" />
                    <span className="task-cmd" title={sa.description}>
                      <b>{sa.subagentType}</b> {sa.description}
                    </span>
                    <span className="task-exit">
                      {Math.round((sa.durationMs ?? 0) / 1000)}s · {sa.turnCount ?? 0}
                      {t.tasksTurns} · {sa.contextUsagePct ?? 0}%
                    </span>
                    <button
                      className="git-mini danger"
                      onClick={async () => {
                        await invoke("subagent_cancel", { subagentId: sa.subagentId }).catch((e) =>
                          setError(String(e)),
                        );
                        refreshTasks();
                      }}
                    >
                      {t.taskCancel}
                    </button>
                  </div>
                ))}
              </div>
            )}

            {Object.keys(schedTasks).length > 0 && (
              <div className="git-group">
                <div className="git-group-head">
                  <span>{t.tasksSched(Object.keys(schedTasks).length)}</span>
                </div>
                {Object.values(schedTasks).map((s: any) => (
                  <div key={s.taskId} className="task-row">
                    <span className="task-dot run" />
                    <span className="task-cmd" title={s.prompt}>
                      <b>{s.humanSchedule}</b> {s.prompt}
                    </span>
                    {s.nextFireAt && (
                      <span className="task-exit" title={s.nextFireAt}>
                        {t.tasksNextFire} {new Date(s.nextFireAt).toLocaleTimeString()}
                      </span>
                    )}
                    <button
                      className="git-mini danger"
                      onClick={() =>
                        // 删除成功后引擎会发 scheduled_task_deleted，由它移除该行；
                        // 这里不做乐观删除，免得和通知重复处理。
                        invoke("scheduler_delete", { taskId: s.taskId }).catch((e) =>
                          setError(String(e)),
                        )
                      }
                    >
                      {t.taskCancel}
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div className="modal-footer">
              <span />
              <button onClick={() => setShowTasks(false)}>{t.close}</button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
