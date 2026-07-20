/* v0.13 拆分：首页空状态（建议 chips + 跨项目近期会话）与计划步骤面板。
   步 A 透传。红线：首页跨项目列表刻意排除当前工作区——左栏已经列了，
   重复列就是两栏打架（踩过）。buildSuggestions/baseName 为 App 层纯函数，以 prop 注入。 */
import { IconClipboard, IconGitBranch } from "../../icons";

export function Home(props: Record<string, any>) {
  const { buildSuggestions, baseName, fileList, gitInfo, items, busy, onComposerChange, otherRecent, planSteps, sessionId, setInput, startSession, taRef, t } = props;
  return (
    <>
      {items.length === 0 && !busy && (
        <div className="empty-state">
          <div className="empty-logo">W</div>
          <div className="empty-title">{t.appTagline}</div>

          {/* 建议来自当前工作区（有改动就先建议审查改动，有 README 才建议总结…）
              工作区信息不在这里重复 —— 左栏底部和输入框上方已经显示。 */}
          {sessionId && (
            <div className="chips">
              {buildSuggestions(fileList, gitInfo, t).map((s: any) => (
                <button
                  key={s.label}
                  className="chip"
                  onClick={() => {
                    setInput(s.prompt);
                    onComposerChange(s.prompt);
                    taRef.current?.focus();
                  }}
                >
                  {s.label}
                </button>
              ))}
            </div>
          )}

          {/* 跨工作区的近期会话。刻意排除当前工作区——那部分左栏已经列了，
              首页再列一遍就是和左栏打架（之前踩过）。这里只回答左栏答不了的
              问题：我在别的项目干到哪了。 */}
          {otherRecent.length > 0 && (
            <div className="home-recent">
              <div className="home-recent-head">{t.homeOtherProjects}</div>
              {otherRecent.map((s: any) => (
                <button
                  key={s.sessionId}
                  className="home-recent-row"
                  title={s.path}
                  onClick={() => startSession(s.sessionId, s.path)}
                >
                  <span className="home-recent-proj">{baseName(s.path)}</span>
                  <span className="home-recent-title">
                    {s.title || t.untitledSession}
                  </span>
                  {s.branch && (
                    <span className="home-recent-branch">
                      <IconGitBranch size={11} /> {s.branch}
                    </span>
                  )}
                </button>
              ))}
            </div>
          )}

          <div className="empty-hint">{t.emptyHint}</div>
        </div>
      )}

      {planSteps.length > 0 && (
        <div className="plan-panel">
          <div className="plan-head"><IconClipboard size={14} /> {t.planTitle}</div>
          {planSteps.map((p: any, i: any) => (
            <div key={i} className={`plan-step ${p.status ?? ""}`}>
              <span className="plan-mark">
                {p.status === "completed" ? "✅" : p.status === "in_progress" ? "▶" : "○"}
              </span>
              <span>{p.content}</span>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
