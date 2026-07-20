/* v0.13 拆分：终端面板（只读输出页 + 交互 PTY 页）。
   注意：PTY 切页只做 CSS 隐藏不卸载（卸载会 kill shell）。 */
import { IconTerminal } from "../../icons";
import PtyTerm from "../../PtyTerm";

export function TerminalPanel(props: Record<string, any>) {
  const { lang, ptyOpened, sessionId, setError, setPtyOpened, setShowTerminal, setTermTab, setTerminalLines, showTerminal, termTab, terminalLines, theme, t } = props;
  return (
    <>
      {showTerminal && (
        <div className="terminal-panel">
          <div className="terminal-head">
            <span className="panel-title"><IconTerminal size={14} /> {lang === "zh" ? "终端" : "Terminal"}</span>
            {/* Agent 的命令输出（只读）和可交互 shell 是两回事，分开两页 */}
            <div className="term-tabs">
              <button
                className={`term-tab ${termTab === "output" ? "on" : ""}`}
                onClick={() => setTermTab("output")}
              >
                {t.termTabOutput}
              </button>
              <button
                className={`term-tab ${termTab === "shell" ? "on" : ""}`}
                onClick={() => {
                  setPtyOpened(true);
                  setTermTab("shell");
                }}
              >
                {t.termTabShell}
              </button>
            </div>
            <div>
              {termTab === "output" && (
                <button className="ghost small" title={lang === "zh" ? "清空" : "Clear"} onClick={() => setTerminalLines([])}>
                  🧹
                </button>
              )}
              <button className="ghost small" onClick={() => setShowTerminal(false)}>
                ✕
              </button>
            </div>
          </div>
          {termTab === "output" && (
            <pre className="terminal-body">
              {terminalLines.length ? terminalLines.join("\n") : lang === "zh" ? "（暂无命令输出）" : "(no command output yet)"}
            </pre>
          )}
          {/* 一旦开过就保持挂载，只用 CSS 藏起来：卸载会 kill 掉 PTY，
              切一下标签就把用户敲了一半的命令和整个 shell 会话丢了。
              key 绑 sessionId：换会话时重建，避免旧 PTY 的输出串进新会话。 */}
          {ptyOpened && (
            <div className="pty-wrap" style={{ display: termTab === "shell" ? "flex" : "none" }}>
              <PtyTerm key={sessionId} sessionKey={sessionId} dark={theme !== "light"} onError={setError} />
            </div>
          )}
        </div>
      )}
    </>
  );
}
