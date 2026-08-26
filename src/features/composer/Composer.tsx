/* v0.13 拆分：底部输入区（排队条/图片条/@弹窗/输入框/加号菜单/模型切换/模式菜单/发送区）。
   步 A 透传。红线：
   - 队列编辑不做乐观更新，引擎 queue/changed 广播回来才刷新（版本守卫是良性 no-op）；
   - ↑/↓ 历史调取只在无候选弹窗时接管，histIdxRef/draftRef 语义保持在 App 层。 */
import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { activateOnKeyboard } from "../../accessibility";
import { assertNever, type AmbiguousCandidate, type ModelBlock } from "../../modelBlock";
import {
  IconArrowUp, IconCheck, IconChevron, IconClipboard, IconFile, IconFolder,
  IconGitBranch, IconPlus, IconShield, IconStop, IconTerminal, IconX,
} from "../../icons";

/// The engine refuses a model id that maps to several catalog entries and
/// returns the candidates instead of picking one. `String(err)` on that object
/// would render "[object Object]", so the shape is narrowed explicitly here and
/// anything else falls back to a readable message.
type ModelSwitchError =
  | { kind: "ambiguous_model_id"; requested: string; candidates: AmbiguousCandidate[] }
  | { kind: "error"; message: string };

function asModelSwitchError(err: unknown): ModelSwitchError {
  const e = err as any;
  if (e && typeof e === "object" && e.kind === "ambiguous_model_id" && Array.isArray(e.candidates)) {
    return e as ModelSwitchError;
  }
  if (e && typeof e === "object" && typeof e.message === "string") {
    return { kind: "error", message: e.message };
  }
  return { kind: "error", message: String(err) };
}

export function Composer(props: Record<string, any>) {
  const { surface, MODE_ORDER, acceptPopup, busy, currentEffort, draftRef, editingQueueId, effortOptions, fileInputRef, histIdxRef, historyRef, input, lang, model, modeMenu, modeMeta, modelBlock, modelBlockOpen, setModelBlock, setModelBlockOpen, modelOptions, models, onAttachWorkFile, onComposerChange, onEffortChange, onModelSwitched, onPaste, onPickImages, pastedImages, permMode, pickFolderAndConnect, plusMenu, popup, popupItems, queue, refreshMcpConfig, send, sendInterject, sessionId, setEditingQueueId, setError, setInput, setItems, setMode, setModeMenu, setModel, setPastedImages, setPlusMenu, setPopup, setSettingsTab, setShowSettings, setShowTerminal, starting, taRef, workspace, t, transcriptMode, setTranscriptMode } = props;

  // Non-null while the engine is waiting for the user to disambiguate a model
  // id. The select is rolled back to `previous` so the dropdown never shows a
  // model the session is not actually on.
  const [ambiguity, setAmbiguity] = useState<
    { requested: string; candidates: AmbiguousCandidate[]; previous: string } | null
  >(null);

  // 会话加载时引擎判定的歧义，与本地一次切换失败导致的歧义，走同一个选择器。
  // 前者才是主路径：下拉里的 option value 是唯一 catalog key，精确 key 永远
  // 不歧义，所以正常从下拉选是触发不了的——真正需要选择器的是"恢复一个只存了
  // 重复 slug 的旧会话"。只接后者等于把选择器接在几乎不会走到的入口上。
  const block: ModelBlock | null = modelBlock ?? null;
  // 穷举而非条件判断：新增一类阻塞却忘了给它 UI，assertNever 会让编译当场
  // 失败。上一版用的是 if/else，所以 model_unavailable 加进来时编译器一声
  // 不吭，那类会话就掉进了无反馈死区。
  const blockView: {
    ambiguity: { requested: string; candidates: AmbiguousCandidate[]; previous: string } | null;
    notice: { title: string; hint: string; subject: string } | null;
  } = (() => {
    if (!block) return { ambiguity: null, notice: null };
    switch (block.kind) {
      case "ambiguous_model_id":
        return {
          ambiguity: { requested: block.requested, candidates: block.candidates, previous: model },
          notice: null,
        };
      case "model_unavailable":
        return {
          ambiguity: null,
          notice: { title: t.unavailableTitle, hint: t.unavailableHint, subject: block.requested },
        };
      case "unknown":
        return {
          ambiguity: null,
          notice: { title: t.blockUnknownTitle, hint: t.blockUnknownHint, subject: block.raw },
        };
      default:
        return assertNever(block);
    }
  })();
  const sessionAmbiguity = blockView.ambiguity;
  // 会话级阻塞可以收起，但收起不是解除——阻塞在引擎里，只有真正选定模型
  // 才会消失。收起后仍留一条常驻提示与重开入口，且发送保持禁用。
  const shownAmbiguity = ambiguity ?? (modelBlockOpen ? sessionAmbiguity : null);
  // 任何一类阻塞都禁发送。上一版这里只认歧义，于是 model_unavailable 的会话
  // 按钮看着能点、点了被 App 的 send() 静默吞掉——用户得不到任何解释。
  const sessionBlocked = !!block;
  const shownNotice = modelBlockOpen ? blockView.notice : null;
  const composingRef = useRef(false);
  const atPopupEmpty = popup?.kind === "at" && popupItems.length === 0;

  async function switchModel(target: string, previous: string) {
    try {
      await invoke("agent_set_model", { model: target });
      setAmbiguity(null);
      // 选定即解除会话阻塞——引擎那边成功切换后 block 已清，本地状态同步。
      setModelBlock?.(null);
      // C2：切换成功后按新模型的能力位推导强度选择器（引擎不回推菜单，
      // 当前档由随后的 ModelChanged 广播校准）。
      onModelSwitched?.(target);
    } catch (err) {
      const e = asModelSwitchError(err);
      setModel(previous);
      if (e.kind === "ambiguous_model_id") {
        setAmbiguity({ requested: e.requested, candidates: e.candidates, previous });
      } else {
        // 普通失败：上一次的候选已经过期，留着会让用户对着一份不再适用的
        // 列表做选择。会话级 block 不清——它不归这次切换管。
        setAmbiguity(null);
        setError(e.message);
      }
    }
  }
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
            {/* Claude Code 式排队行：整行即文本（点击进入行内编辑），
                动作只保留两个且 hover 才现身——⏎ 立即插话、✕ 删除。
                排序按钮移除（低频操作不值得常驻按钮位）。 */}
            {queue.map((q: any) => (
              <div key={q.id} className="queue-row">
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
                  <span
                    className="queue-text"
                    title={t.queueEdit}
                    role="button"
                    tabIndex={0}
                    onClick={() => setEditingQueueId(q.id)}
                    onKeyDown={(event) =>
                      activateOnKeyboard(event, () => setEditingQueueId(q.id))
                    }
                  >
                    {q.text}
                  </span>
                )}
                <span className="queue-actions">
                  <button
                    className="icon-btn queue-x"
                    title={t.queueInterjectNow}
                    onClick={() =>
                      // 立即插话：不等回合结束，当前回合内注入执行。
                      // 版本守卫：过期就是良性 no-op + 引擎重播队列。
                      invoke("agent_queue_interject", {
                        id: q.id,
                        expectedVersion: q.version,
                      }).catch((e) => setError(String(e)))
                    }
                  >
                    ⏎
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
                </span>
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
          {(popup && popupItems.length > 0) || atPopupEmpty ? (
            <div className="mention-popup">
              {atPopupEmpty ? (
                <div className="mention-item mention-empty">{t.mentionNoFiles}</div>
              ) : (
                popupItems.map((it: any, idx: any) => (
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
                ))
              )}
            </div>
          ) : null}
          <textarea
            ref={taRef}
            value={input}
            onChange={(e) => onComposerChange(e.currentTarget.value, composingRef.current)}
            onCompositionStart={() => {
              composingRef.current = true;
            }}
            onCompositionEnd={(e) => {
              composingRef.current = false;
              onComposerChange(e.currentTarget.value, false);
            }}
            onPaste={onPaste}
            onKeyDown={(e) => {
              // IME confirmation (Space/Enter) must not send or steal the key.
              if (e.nativeEvent.isComposing || e.keyCode === 229) return;
              const visiblePopup = !!(popup && popupItems.length > 0);
              if (visiblePopup) {
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
              if (e.key === "ArrowUp" && !visiblePopup && historyRef.current.length > 0) {
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
              if (e.key === "ArrowDown" && !visiblePopup && histIdxRef.current >= 0) {
                e.preventDefault();
                const next = histIdxRef.current - 1;
                histIdxRef.current = next;
                onComposerChange(next < 0 ? draftRef.current : historyRef.current[next]);
                return;
              }
              // 忙时默认对齐 Claude Code：普通 Enter = 注入当前回合
              //（不等回合结束，下一个安全点送达模型）；Alt+Enter = 排队到
              // 回合结束后作为新回合。带图片时 interject 不支持图片，退回排队。
              if (e.key === "Enter" && e.altKey && busy) {
                e.preventDefault();
                histIdxRef.current = -1;
                send(); // 显式排队（引擎 FIFO，回合结束后按序执行）
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
                if (busy && pastedImages.length === 0 && input.trim()) {
                  sendInterject(); // 对齐 Claude Code：忙时消息注入当前回合
                } else {
                  send();
                }
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
                      {(surface === "code" || surface === "work") && <button className="plus-item" onClick={pickFolderAndConnect}>
                        <IconFolder size={15} /> {t.menuOpenFolder}
                      </button>}
                      {surface === "work" && <button
                        className="plus-item"
                        disabled={!workspace}
                        onClick={() => {
                          setPlusMenu(false);
                          onAttachWorkFile?.();
                        }}
                      >
                        <IconFile size={15} /> {t.menuAddFile}
                      </button>}
                      {surface !== "work" && <button
                        className="plus-item"
                        disabled={!sessionId}
                        onClick={() => {
                          setPlusMenu(false);
                          fileInputRef.current?.click();
                        }}
                      >
                        <IconFile size={15} /> {t.menuAddImage}
                      </button>}
                      {surface === "code" && <button
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
                      </button>}
                      {surface === "code" && <button
                        className="plus-item"
                        onClick={() => {
                          setPlusMenu(false);
                          refreshMcpConfig();
                          setSettingsTab("mcp");
                          setShowSettings(true);
                        }}
                      >
                        <IconGitBranch size={15} /> {t.menuMcp}
                      </button>}
                    </div>
                  </>
                )}
              </div>
              {surface === "chat" ? (
                <span className="ws-inline"><span className="dot" />{t.surfaceChat}</span>
              ) : workspace ? (
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
              <span className="model-wrap">
              <select
                className="composer-model"
                value={model}
                title={t.modelSwitchHint}
                onChange={(e) => {
                  const m = e.currentTarget.value;
                  const previous = model;
                  setModel(m);
                  // Live switch — no restart, keeps conversation context.
                  if (sessionId) void switchModel(m, previous);
                }}
              >
                {(() => {
                  // 按 id 合并两个来源（复核 P1）：modelOptions 只在会话启动时
                  // 拿到一次，而热加载新模型后引擎推的是 models 裸 id 列表。
                  // 只认结构化列表会把 v0.18.5 修过的"保存新模型必须重启"
                  // 请回来——结构化条目优先，models 里缺失的 key 以裸 key 补齐。
                  const structured = modelOptions?.length ? modelOptions : [];
                  const known = new Set(structured.map((o: { id: string }) => o.id));
                  const bare = (models.length || structured.length
                    ? models
                    : ["glm-5.2", "glm-5-turbo", "glm-4-flash", "deepseek-chat", "deepseek-reasoner"]
                  )
                    .filter((m: string) => !known.has(m))
                    .map((m: string) => ({ id: m, name: m, endpointLabel: "" }));
                  return [...structured, ...bare];
                })().map((o: { id: string; name: string; endpointLabel: string }) => (
                  <option key={o.id} value={o.id}>
                    {/* value 永远是 catalog key；同名模型靠端点区分 */}
                    {o.endpointLabel ? `${o.name} · ${o.endpointLabel}` : o.name}
                  </option>
                ))}
              </select>
              {/* C2：推理强度选择器——只在引擎下发菜单时渲染（unknown ≠
                  advertised）。切强度走与切模型同一条 setModel 事务。 */}
              {sessionId && effortOptions?.length > 0 && (
                <select
                  className="composer-model composer-effort"
                  value={currentEffort ?? ""}
                  title={t.effortHint}
                  onChange={(e) => {
                    const v = e.currentTarget.value;
                    if (v) onEffortChange?.(v);
                  }}
                >
                  {currentEffort == null && (
                    <option value="" disabled>
                      {t.effortDefault}
                    </option>
                  )}
                  {effortOptions.map((o: { id: string; label: string }) => (
                    <option key={o.id} value={o.id}>
                      {o.label}
                    </option>
                  ))}
                </select>
              )}
              {shownNotice && (
                <div className="model-ambiguity" role="dialog" aria-label={shownNotice.title}>
                  <div className="ma-title">{shownNotice.title}</div>
                  <div className="ma-hint">
                    <code>{shownNotice.subject}</code> — {shownNotice.hint}
                  </div>
                  {block?.kind === "model_unavailable" && sessionId && models.includes(model) && (
                    <button
                      className="ma-item"
                      onClick={() => {
                        // 只剩一个模型时，下拉已经显示它，再选择同一 value 不会
                        // 触发 onChange。显式确认按钮仍走同一条严格切换事务，
                        // 成功后才清 block，避免用户被永久困在不可发送状态。
                        void switchModel(model, model);
                      }}
                    >
                      {t.unavailableUseCurrent}
                    </button>
                  )}
                  <button className="ma-cancel" onClick={() => setModelBlockOpen?.(false)}>
                    {t.ambiguousDismiss}
                  </button>
                </div>
              )}
              {sessionBlocked && !modelBlockOpen && (
                <button className="model-blocked-chip" onClick={() => setModelBlockOpen?.(true)}>
                  {t.ambiguousBlocked} · {t.ambiguousReopen}
                </button>
              )}
              {shownAmbiguity && (
                <div className="model-ambiguity" role="dialog" aria-label={t.ambiguousTitle}>
                  <div className="ma-title">{t.ambiguousTitle}</div>
                  <div className="ma-hint">
                    <code>{shownAmbiguity.requested}</code> — {t.ambiguousHint}
                  </div>
                  <div className="ma-list">
                    {shownAmbiguity.candidates.map((c: AmbiguousCandidate) => (
                      <button
                        key={c.id}
                        className="ma-item"
                        disabled={!c.selectable}
                        title={c.selectable ? c.endpointLabel : t.ambiguousUnavailable}
                        onClick={() => {
                          setModel(c.id);
                          void switchModel(c.id, shownAmbiguity.previous);
                        }}
                      >
                        <span className="ma-name">{c.name}</span>
                        <span className="ma-endpoint">{c.endpointLabel}</span>
                        {!c.selectable && <span className="ma-locked">{t.ambiguousUnavailable}</span>}
                      </button>
                    ))}
                  </div>
                  <button
                    className="ma-cancel"
                    onClick={() => {
                      // 本地一次切换失败：真取消，回到原模型即可。
                      // 会话级阻塞：只收起弹窗，绝不清 modelBlock——引擎那边
                      // 的 block 还在，前端假装解除只会让用户点发送后收到一个
                      // 空 EndTurn，比不给取消按钮更糟。
                      if (ambiguity) setAmbiguity(null);
                      else setModelBlockOpen?.(false);
                    }}
                  >
                    {ambiguity ? t.ambiguousCancel : t.ambiguousDismiss}
                  </button>
                </div>
              )}
              </span>
            </div>
            <div className="composer-actions">
              <div className="density-control" role="group" aria-label={t.densityTitle}>
                {(["compact", "default", "verbose"] as const).map((m) => (
                  <button
                    key={m}
                    type="button"
                    className={(transcriptMode || "default") === m ? "active" : ""}
                    title={m === "compact" ? t.densityCompact : m === "verbose" ? t.densityVerbose : t.densityQuiet}
                    onClick={() => setTranscriptMode?.(m)}
                  >
                    {m === "compact" ? t.densityCompact : m === "verbose" ? t.densityVerbose : t.densityQuiet}
                  </button>
                ))}
              </div>
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
                      <div className="mode-menu-sep" role="separator" />
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
              {surface === "code" && sessionId && (
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
                  disabled={sessionBlocked || starting || (!input.trim() && pastedImages.length === 0)}
                  title={sessionBlocked ? t.ambiguousBlocked : t.send}
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
