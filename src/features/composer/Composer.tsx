/* v0.13 拆分：底部输入区（排队条/图片条/@弹窗/输入框/加号菜单/模型切换/模式菜单/发送区）。
   步 A 透传。红线：
   - 队列编辑不做乐观更新，引擎 queue/changed 广播回来才刷新（版本守卫是良性 no-op）；
   - ↑/↓ 历史调取只在无候选弹窗时接管，histIdxRef/draftRef 语义保持在 App 层。 */
import { invoke } from "@tauri-apps/api/core";
import {
  IconArrowUp, IconCheck, IconChevron, IconClipboard, IconFile, IconFolder,
  IconGitBranch, IconPencil, IconPlus, IconShield, IconStop, IconTerminal, IconX,
} from "../../icons";

export function Composer(props: Record<string, any>) {
  const { MODE_ORDER, acceptPopup, busy, draftRef, editingQueueId, fileInputRef, histIdxRef, historyRef, input, lang, model, modeMenu, modeMeta, models, onComposerChange, onPaste, onPickImages, pastedImages, permMode, pickFolderAndConnect, plusMenu, popup, popupItems, queue, refreshMcpConfig, send, sendInterject, sessionId, setEditingQueueId, setError, setInput, setItems, setMode, setModeMenu, setModel, setPastedImages, setPlusMenu, setPopup, setSettingsTab, setShowSettings, setShowTerminal, starting, taRef, workspace, t } = props;
  return (
    <>
      <input
        ref={fileInputRef}
        type="file"
        accept="image/*"
        multiple
        style={{ display: "none" }}
        onChange={onPickImages}
      />
      <footer className="composer">
        {/* 排队中的提示词：Agent 忙时输入不再被拦，引擎按 FIFO 依次执行 */}
        {queue.length > 0 && (
          <div className="queue-strip">
            <div className="queue-head">
              <span className="queue-title">{t.queueTitle(queue.length)}</span>
              <button
                className="queue-clear"
                onClick={() => invoke("agent_queue_clear").catch((e) => setError(String(e)))}
              >
                {t.queueClear}
              </button>
            </div>
            {queue.map((q: any, n: any) => (
              <div key={q.id} className="queue-row">
                <span className="queue-idx">{n + 1}</span>
                {editingQueueId === q.id ? (
                  <input
                    className="queue-edit-input"
                    autoFocus
                    defaultValue={q.text}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        const v = e.currentTarget.value.trim();
                        setEditingQueueId(null);
                        // 引擎确认后经 queue/changed 广播回来刷新文本，不乐观更新
                        if (v && v !== q.text)
                          invoke("agent_queue_edit", { id: q.id, newText: v }).catch((err) =>
                            setError(String(err)),
                          );
                      } else if (e.key === "Escape") setEditingQueueId(null);
                    }}
                    onBlur={() => setEditingQueueId(null)}
                  />
                ) : (
                  <span className="queue-text" title={q.text}>
                    {q.text}
                  </span>
                )}
                <button
                  className="icon-btn queue-x"
                  title={t.queueMoveUp}
                  disabled={n === 0}
                  onClick={() => {
                    const ids = queue.map((x: any) => x.id);
                    [ids[n - 1], ids[n]] = [ids[n], ids[n - 1]];
                    invoke("agent_queue_reorder", { orderedIds: ids }).catch((e) =>
                      setError(String(e)),
                    );
                  }}
                >
                  ↑
                </button>
                <button
                  className="icon-btn queue-x"
                  title={t.queueMoveDown}
                  disabled={n === queue.length - 1}
                  onClick={() => {
                    const ids = queue.map((x: any) => x.id);
                    [ids[n], ids[n + 1]] = [ids[n + 1], ids[n]];
                    invoke("agent_queue_reorder", { orderedIds: ids }).catch((e) =>
                      setError(String(e)),
                    );
                  }}
                >
                  ↓
                </button>
                <button
                  className="icon-btn queue-x"
                  title={t.queueEdit}
                  onClick={() => setEditingQueueId(q.id)}
                >
                  <IconPencil size={12} />
                </button>
                <button
                  className="icon-btn queue-x"
                  title={t.queueInterjectNow}
                  onClick={() =>
                    // 立即插话：这条排队消息不等回合结束，当前回合内注入执行。
                    // 版本守卫同 remove——过期就是良性 no-op + 引擎重播队列。
                    invoke("agent_queue_interject", { id: q.id, expectedVersion: q.version }).catch(
                      (e) => setError(String(e)),
                    )
                  }
                >
                  ⚡
                </button>
                <button
                  className="icon-btn queue-x"
                  title={t.queueRemove}
                  onClick={() =>
                    invoke("agent_queue_remove", { id: q.id, expectedVersion: q.version }).catch(
                      (e) => setError(String(e)),
                    )
                  }
                >
                  <IconX size={13} />
                </button>
              </div>
            ))}
          </div>
        )}
        <div className="composer-input-wrap">
          {pastedImages.length > 0 && (
            <div className="image-strip">
              {pastedImages.map((im: any, i: any) => (
                <div key={i} className="image-thumb">
                  <img src={im.preview} alt="" />
                  <button
                    title={t.removeImage}
                    onClick={() => setPastedImages((prev: any) => prev.filter((_: any, j: any) => j !== i))}
                  >
                    <IconX size={12} />
                  </button>
                </div>
              ))}
            </div>
          )}
          {popup && popupItems.length > 0 && (
            <div className="mention-popup">
              {popupItems.map((it: any, idx: any) => (
                <div
                  key={it.label}
                  className={`mention-item ${idx === popup.sel ? "active" : ""}`}
                  onMouseDown={(e) => {
                    e.preventDefault();
                    acceptPopup(idx);
                  }}
                >
                  <span className="mention-label">{it.label}</span>
                  {it.desc && <span className="mention-desc">{it.desc}</span>}
                </div>
              ))}
            </div>
          )}
          <textarea
            ref={taRef}
            value={input}
            onChange={(e) => onComposerChange(e.currentTarget.value)}
            onPaste={onPaste}
            onKeyDown={(e) => {
              if (popup && popupItems.length > 0) {
                if (e.key === "ArrowDown") {
                  e.preventDefault();
                  setPopup({ ...popup, sel: (popup.sel + 1) % popupItems.length });
                  return;
                }
                if (e.key === "ArrowUp") {
                  e.preventDefault();
                  setPopup({ ...popup, sel: (popup.sel - 1 + popupItems.length) % popupItems.length });
                  return;
                }
                if (e.key === "Enter" || e.key === "Tab") {
                  e.preventDefault();
                  acceptPopup(popup.sel);
                  return;
                }
                if (e.key === "Escape") {
                  e.preventDefault();
                  setPopup(null);
                  return;
                }
              }
              // ↑/↓ 调取历史输入：只在没有候选弹窗、且不是在多行文本里移动光标时接管。
              if (e.key === "ArrowUp" && !popup && historyRef.current.length > 0) {
                const atStart = e.currentTarget.selectionStart === 0;
                if (input === "" || histIdxRef.current >= 0 || atStart) {
                  e.preventDefault();
                  if (histIdxRef.current < 0) draftRef.current = input; // 存草稿
                  const next = Math.min(histIdxRef.current + 1, historyRef.current.length - 1);
                  histIdxRef.current = next;
                  onComposerChange(historyRef.current[next] ?? "");
                  return;
                }
              }
              if (e.key === "ArrowDown" && !popup && histIdxRef.current >= 0) {
                e.preventDefault();
                const next = histIdxRef.current - 1;
                histIdxRef.current = next;
                onComposerChange(next < 0 ? draftRef.current : historyRef.current[next]);
                return;
              }
              // Alt+Enter：忙时插话（不打断当前回合）
              if (e.key === "Enter" && e.altKey && busy) {
                e.preventDefault();
                sendInterject();
                return;
              }
              // Shift+Tab：切换计划模式（对标 Claude Code 的模式循环键）。
              // 走引擎的 toggle 通知，它回发 current_mode_update，UI 跟随。
              if (e.key === "Tab" && e.shiftKey && sessionId) {
                e.preventDefault();
                invoke("agent_toggle_plan_mode").catch(() => {});
                return;
              }
              if (e.key === "Enter" && !e.shiftKey && !e.altKey) {
                e.preventDefault();
                histIdxRef.current = -1;
                send();
              }
            }}
            placeholder={
              busy
                ? t.queueHint
                : sessionId
                  ? t.composerPlaceholder
                  : starting
                    ? t.starting
                    : t.composerHint
            }
            rows={2}
          />
          <div className="composer-bar">
            <div className="composer-left">
              <div className="plus-wrap">
                <button
                  className="icon-btn plus-btn"
                  title={t.addMenu}
                  onClick={() => setPlusMenu((v: any) => !v)}
                >
                  <IconPlus size={18} />
                </button>
                {plusMenu && (
                  <>
                    <div className="plus-backdrop" onClick={() => setPlusMenu(false)} />
                    <div className="plus-menu">
                      <button className="plus-item" onClick={pickFolderAndConnect}>
                        <IconFolder size={15} /> {t.menuOpenFolder}
                      </button>
                      <button
                        className="plus-item"
                        disabled={!sessionId}
                        onClick={() => {
                          setPlusMenu(false);
                          fileInputRef.current?.click();
                        }}
                      >
                        <IconFile size={15} /> {t.menuAddImage}
                      </button>
                      <button
                        className="plus-item"
                        disabled={!sessionId}
                        onClick={() => {
                          setPlusMenu(false);
                          setInput("/");
                          onComposerChange("/");
                          taRef.current?.focus();
                        }}
                      >
                        <IconClipboard size={15} /> {t.menuSlash}
                      </button>
                      <button
                        className="plus-item"
                        onClick={() => {
                          setPlusMenu(false);
                          refreshMcpConfig();
                          setSettingsTab("mcp");
                          setShowSettings(true);
                        }}
                      >
                        <IconGitBranch size={15} /> {t.menuMcp}
                      </button>
                    </div>
                  </>
                )}
              </div>
              {sessionId ? (
                <span className="ws-inline" title={workspace}>
                  <span className="dot" />
                  {workspace.split(/[\\/]/).filter(Boolean).pop()}
                </span>
              ) : (
                <button className="ws-inline connect" onClick={pickFolderAndConnect} disabled={starting}>
                  <IconFolder size={13} />
                  {starting ? t.starting : t.openWorkspace}
                </button>
              )}
              <select
                className="composer-model"
                value={model}
                title={t.modelSwitchHint}
                onChange={(e) => {
                  const m = e.currentTarget.value;
                  setModel(m);
                  // Live switch — no restart, keeps conversation context.
                  if (sessionId) invoke("agent_set_model", { model: m }).catch((err) => setError(String(err)));
                }}
              >
                {(models.length ? models : ["glm-5.2", "glm-5-turbo", "glm-4-flash", "deepseek-chat", "deepseek-reasoner"]).map(
                  (m: any) => (
                    <option key={m} value={m}>
                      {m}
                    </option>
                  ),
                )}
              </select>
              <div className="mode-wrap">
                <button
                  className="mode-chip"
                  data-mode={permMode}
                  title={t.modeMenuTitle}
                  onClick={() => setModeMenu((v: any) => !v)}
                >
                  <IconShield size={13} /> {modeMeta[permMode].label}
                  <IconChevron size={12} />
                </button>
                {modeMenu && (
                  <>
                    <div className="plus-backdrop" onClick={() => setModeMenu(false)} />
                    <div className="mode-menu">
                      <div className="mode-menu-head">{t.modeMenuTitle}</div>
                      {MODE_ORDER.map((m: any) => (
                        <button
                          key={m}
                          className={`mode-item ${permMode === m ? "active" : ""}`}
                          data-mode={m}
                          onClick={() => {
                            setModeMenu(false);
                            setMode(m);
                          }}
                        >
                          <span className="mode-item-text">
                            <span className="mode-item-label">{modeMeta[m].label}</span>
                            <span className="mode-item-desc">{modeMeta[m].desc}</span>
                          </span>
                          {permMode === m && <IconCheck size={15} className="mode-item-check" />}
                        </button>
                      ))}
                      <button
                        className="mode-item mode-reset"
                        onClick={() => {
                          setModeMenu(false);
                          invoke("permissions_reset")
                            .then(() =>
                              setItems((prev: any) => [
                                ...prev,
                                { kind: "note", text: t.permResetDone },
                              ]),
                            )
                            .catch((e) => setError(String(e)));
                        }}
                      >
                        <span className="mode-item-text">
                          <span className="mode-item-label">{t.permReset}</span>
                          <span className="mode-item-desc">{t.permResetDesc}</span>
                        </span>
                      </button>
                    </div>
                  </>
                )}
              </div>
            </div>
            <div className="composer-actions">
              {sessionId && (
                <button
                  className="icon-btn"
                  title={lang === "zh" ? "终端" : "Terminal"}
                  onClick={() => setShowTerminal((s: any) => !s)}
                >
                  <IconTerminal size={15} />
                </button>
              )}
              {busy ? (
                <>
                  {/* 插话：不打断也不排队，当前回合内注入引导（Alt+Enter） */}
                  <button
                    className="send-btn interject"
                    onClick={sendInterject}
                    disabled={!input.trim()}
                    title={t.interjectTitle}
                  >
                    ⚡
                  </button>
                  <button
                    className="send-btn stop"
                    onClick={() => invoke("agent_cancel").catch(() => {})}
                    title={t.stopTitle}
                  >
                    <IconStop size={16} />
                  </button>
                </>
              ) : (
                <button
                  className="send-btn"
                  onClick={send}
                  disabled={starting || (!input.trim() && pastedImages.length === 0)}
                  title={t.send}
                >
                  <IconArrowUp size={16} />
                </button>
              )}
            </div>
          </div>
        </div>
      </footer>
    </>
  );
}
