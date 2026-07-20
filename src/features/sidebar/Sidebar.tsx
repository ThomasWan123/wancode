/* v0.13 拆分：左侧栏（导航/最近会话/文件树/搜索/工作区切换）。步 A 透传。
   红线：工作区标签以会话真实 cwd 为准（#83），不要自行从 localStorage 推导。 */
import { invoke } from "@tauri-apps/api/core";
import {
  IconFolder, IconGitBranch, IconPlus, IconSearch, IconChevron,
  IconFile, IconFolderClosed, IconSettings,  IconPencil, IconTrash, IconClipboard,
} from "../../icons";

export function Sidebar(props: Record<string, any>) {
  const { sessionIdRef, TreeView, buildTree, fileList, gitInfo, grepHits, grepQuery, grepping, knownWorkspaces, mcpLive, mcpServers, pickFolderAndConnect, refreshMcpConfig, refreshMcpLive, refreshSessions, refreshSkills, refreshWorkspaces, runGrep, runSearch, searchHits, searchQuery, sessionId, sessions, setError, setGrepHits, setGrepQuery, setInput, setItems, setSessionId, setSettingsTab, setShowSearch, setShowSettings, setSidebarTab, setWorkspace, setWsMenu, showSearch, sidebarTab, startSession, starting, workspace, wsMenu, t } = props;
  return (
    <>
        <aside className="sidebar">
          {/* 新会话置顶 + 导航 + 最近 —— 层次对标 Claude Code 左栏 */}
          <button
            className="side-new"
            onClick={() => {
              setSessionId("");
              sessionIdRef.current = "";
              setItems([]);
              setSidebarTab("sessions");
            }}
          >
            <IconPlus size={15} /> {t.sidebarNewSession}
          </button>

          <nav className="side-nav">
            <button
              className={`side-nav-item ${sidebarTab === "files" ? "active" : ""}`}
              onClick={() => setSidebarTab(sidebarTab === "files" ? "sessions" : "files")}
            >
              <IconFolderClosed size={15} /> {t.tabFiles}
            </button>
            <button
              className="side-nav-item"
              onClick={async () => {
                setSettingsTab("skills");
                setShowSettings(true);
                refreshSkills();
              }}
            >
              <IconClipboard size={15} /> {t.navSkills}
            </button>
            <button
              className="side-nav-item"
              onClick={() => {
                refreshMcpConfig();
                refreshMcpLive();
                setSettingsTab("mcp");
                setShowSettings(true);
              }}
            >
              <IconGitBranch size={15} /> {t.navMcp}
              {/* 有实时数据时数"真正启用的"，而不是配置文件里的条目数 ——
                  被文件夹信任挡住的服务器不该被算成可用。 */}
              {(mcpLive.length ? mcpLive.filter((s: any) => s.session?.enabled !== false).length : mcpServers.length) > 0 && (
                <span className="side-nav-count">
                  {mcpLive.length
                    ? mcpLive.filter((s: any) => s.session?.enabled !== false).length
                    : mcpServers.length}
                </span>
              )}
            </button>
            <button
              className="side-nav-item"
              onClick={() => {
                setSettingsTab("general");
                setShowSettings(true);
              }}
            >
              <IconSettings size={15} /> {t.settings}
            </button>
          </nav>

          {sidebarTab === "files" ? (
            <>
              {/* 项目内容搜索 —— 引擎侧 ripgrep 语义，尊重 .gitignore */}
              <input
                className="session-search"
                value={grepQuery}
                placeholder={t.grepPlaceholder}
                onChange={(e) => setGrepQuery(e.currentTarget.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") runGrep();
                  if (e.key === "Escape") {
                    setGrepQuery("");
                    setGrepHits(null);
                  }
                }}
              />
              <div className="session-list tree-list">
                {grepping && <div className="sidebar-empty">{t.searching}</div>}
                {!grepping && grepHits !== null && grepHits.length === 0 && (
                  <div className="sidebar-empty">{t.grepNoHits}</div>
                )}
                {!grepping &&
                  grepHits !== null &&
                  grepHits.map((f: any) => (
                    <div key={f.path} className="grep-file">
                      <div
                        className="grep-file-head"
                        title={f.path}
                        onClick={() =>
                          setInput((v: any) => v + (v && !v.endsWith(" ") ? " " : "") + "@" + f.path + " ")
                        }
                      >
                        <IconFile size={12} /> {f.name}
                        <span className="grep-count">{f.matches.length}</span>
                      </div>
                      {f.matches.slice(0, 5).map((m: any, i: any) => (
                        <div key={i} className="grep-line" title={m.content}>
                          <span className="grep-lineno">{m.line}</span>
                          <span className="grep-text">{m.content.trim().slice(0, 120)}</span>
                        </div>
                      ))}
                    </div>
                  ))}
                {grepHits === null &&
                  (fileList.length === 0 ? (
                    <div className="sidebar-empty">{sessionId ? "—" : t.emptySubStart}</div>
                  ) : (
                    <TreeView
                      node={buildTree(fileList)}
                      onPick={(p: any) =>
                        setInput((v: any) => v + (v && !v.endsWith(" ") ? " " : "") + "@" + p + " ")
                      }
                    />
                  ))}
              </div>
            </>
          ) : (
          <>
          <div className="side-section">
            <span className="side-section-title">{t.sidebarRecent}</span>
            <button
              className="icon-btn side-section-btn"
              title={t.sidebarSearchToggle}
              onClick={() => {
                const next = !showSearch;
                setShowSearch(next);
                if (!next) runSearch("");
              }}
            >
              <IconSearch size={14} />
            </button>
          </div>
          {showSearch && (
            <input
              className="session-search"
              autoFocus
              value={searchQuery}
              placeholder={t.searchPlaceholder}
              onChange={(e) => runSearch(e.currentTarget.value)}
            />
          )}
          <div className="session-list">
            {searchHits !== null && searchHits.length === 0 && (
              <div className="sidebar-empty">{t.searchNoResults}</div>
            )}
            {searchHits === null && sessions.length === 0 && (
              <div className="sidebar-empty">{t.noSessions}</div>
            )}
            {(searchHits ?? sessions).map((s: any) => (
              <div
                key={s.session_id}
                className={`session-item ${s.session_id === sessionId ? "active" : ""}`}
                onClick={() => !starting && startSession(s.session_id)}
                title={s.session_id}
              >
                <div className="session-row">
                  <div className="session-title">{s.title}</div>
                  <div className="session-actions">
                    <span
                      title={t.renameSession}
                      onClick={async (e) => {
                        e.stopPropagation();
                        const title = window.prompt(t.renameSession, s.title);
                        if (!title?.trim()) return;
                        try {
                          if (!sessionId) await startSession();
                          await invoke("agent_session_rename", {
                            sessionId: s.session_id,
                            title: title.trim(),
                            workspace,
                          });
                          refreshSessions(workspace);
                        } catch (err) {
                          setError(String(err));
                        }
                      }}
                    >
                      <IconPencil size={13} />
                    </span>
                    <span
                      title={t.deleteSession}
                      onClick={async (e) => {
                        e.stopPropagation();
                        if (!window.confirm(t.deleteConfirm(s.title))) return;
                        try {
                          if (!sessionId) await startSession();
                          await invoke("agent_session_delete", {
                            sessionId: s.session_id,
                            workspace,
                          });
                          if (s.session_id === sessionId) {
                            setSessionId("");
                            sessionIdRef.current = "";
                            setItems([]);
                          }
                          refreshSessions(workspace);
                        } catch (err) {
                          setError(String(err));
                        }
                      }}
                    >
                      <IconTrash size={13} />
                    </span>
                  </div>
                </div>
                <div className="session-meta">
                  {s.updated_at.slice(0, 16).replace("T", " ")} · {s.num_messages} {t.messagesUnit}
                  {s.model_id ? ` · ${s.model_id}` : ""}
                </div>
              </div>
            ))}
          </div>
          </>
          )}
          {/* 底部工作区行 —— 点击可跨项目切换（对应 Claude Code 的跨项目 Recents） */}
          <div className="side-foot">
            {wsMenu && (
              <>
                <div className="plus-backdrop" onClick={() => setWsMenu(false)} />
                <div className="ws-menu">
                  <div className="mode-menu-head">{t.wsSwitch}</div>
                  {knownWorkspaces
                    .filter((w: any) => w.path !== workspace)
                    .slice(0, 8)
                    .map((w: any) => (
                      <button
                        key={w.path}
                        className="ws-menu-item"
                        title={w.path}
                        onClick={() => {
                          setWsMenu(false);
                          setWorkspace(w.path);
                          localStorage.setItem("wancode-workspace", w.path);
                          refreshSessions(w.path);
                          startSession(undefined, w.path);
                        }}
                      >
                        <IconFolderClosed size={14} />
                        <span className="ws-menu-name">
                          {w.path.split(/[\\/]/).filter(Boolean).pop()}
                        </span>
                        <span className="ws-menu-count">{w.sessions}</span>
                      </button>
                    ))}
                  <button className="ws-menu-item" onClick={pickFolderAndConnect}>
                    <IconFolder size={14} /> <span className="ws-menu-name">{t.wsBrowse}</span>
                  </button>
                </div>
              </>
            )}
            <button
              className="side-foot-ws"
              onClick={() => {
                if (!workspace) {
                  pickFolderAndConnect();
                  return;
                }
                refreshWorkspaces();
                setWsMenu((v: any) => !v);
              }}
              disabled={starting}
              title={workspace || t.sugOpenFolder}
            >
              <IconFolder size={14} />
              <span className="side-foot-name">
                {workspace ? workspace.split(/[\\/]/).filter(Boolean).pop() : t.sugOpenFolder}
              </span>
              {workspace && <IconChevron size={12} />}
            </button>
            <div className="side-foot-meta">
              {/* gitInfo === null 表示"还没取到/取失败"，不能当成"不是仓库" */}
              {gitInfo?.isRepo ? (
                <>
                  <IconGitBranch size={11} /> {gitInfo.branch}
                  {(gitInfo.files?.length ?? 0) > 0 && ` · ${t.homeChanged(gitInfo.files.length)}`}
                </>
              ) : gitInfo?.isRepo === false ? (
                t.sidebarNoRepo
              ) : null}
            </div>
          </div>
        </aside>
    </>
  );
}
