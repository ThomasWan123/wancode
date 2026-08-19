/* C3（v0.20）：项目记忆设置面。状态自包含——设置页是低频面，不再给
   App 的 prop 袋加码。开关/flush/追加全部直打 Tauri 命令边界。 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function MemoryPanel(props: { sessionId: string; surface: string; workspace: string; t: any }) {
  const { sessionId, surface, workspace, t } = props;
  const [enabled, setEnabled] = useState(false);
  const [globalMem, setGlobalMem] = useState("");
  const [workspaceMem, setWorkspaceMem] = useState<{ dir_name: string; content: string } | null>(null);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState<"" | "flush" | "append">("");
  const [msg, setMsg] = useState("");

  const refresh = useCallback(async () => {
    const errors: string[] = [];
    try { setEnabled(await invoke<boolean>("memory_config_get")); }
    catch (e) { errors.push(String(e)); }
    try { setGlobalMem(await invoke<string>("memory_read_global")); }
    catch (e) { errors.push(String(e)); }
    if (!workspace) setWorkspaceMem(null);
    else {
      try { setWorkspaceMem(await invoke<any>("memory_read_workspace", { workspace })); }
      catch (e) { errors.push(String(e)); }
    }
    if (errors.length > 0) setMsg(errors[0]);
  }, [workspace]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  if (surface !== "code") {
    return <div className="modal-section"><div className="modal-hint">{t.memCodeOnly}</div></div>;
  }

  const toggle = async (next: boolean) => {
    setEnabled(next);
    setMsg("");
    try {
      await invoke("memory_config_set", { enabled: next });
      setMsg(t.memEnableNote);
    } catch (e) {
      setEnabled(!next);
      setMsg(String(e));
    }
  };

  const flush = async () => {
    setBusy("flush");
    setMsg("");
    try {
      await invoke("memory_flush");
      setMsg(t.memFlushOk);
      await refresh();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy("");
    }
  };

  const append = async () => {
    if (!note.trim()) return;
    setBusy("append");
    setMsg("");
    try {
      await invoke("memory_append_global", { text: note.trim() });
      setNote("");
      setMsg(t.memAppended);
      await refresh();
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy("");
    }
  };

  return (
    <div className="modal-section">
      <label className="modal-label" style={{ display: "flex", alignItems: "center", gap: 8 }}>
        <input type="checkbox" checked={enabled} onChange={(e) => void toggle(e.currentTarget.checked)} />
        {t.memEnable}
      </label>
      <div className="modal-hint">{t.memEnableNote}</div>

      <div className="modal-label" style={{ marginTop: 16 }}>{t.memFlushLabel}</div>
      <button disabled={!sessionId || busy !== ""} onClick={() => void flush()}>
        {busy === "flush" ? t.memFlushBusy : t.memFlush}
      </button>
      {!sessionId && <div className="modal-hint">{t.memNoSession}</div>}

      <div className="modal-label" style={{ marginTop: 16 }}>{t.memAddLabel}</div>
      <textarea
        rows={3}
        style={{ width: "100%" }}
        placeholder={t.memAddPlaceholder}
        value={note}
        onChange={(e) => setNote(e.currentTarget.value)}
      />
      <button disabled={!note.trim() || busy !== ""} onClick={() => void append()}>
        {busy === "append" ? t.memAppendBusy : t.memAppend}
      </button>

      {msg && <div className="modal-hint" style={{ marginTop: 8 }}>{msg}</div>}

      <div className="modal-label" style={{ marginTop: 16 }}>{t.memGlobalTitle}</div>
      <pre className="memory-preview">{globalMem || t.memEmpty}</pre>

      <div className="modal-label" style={{ marginTop: 16 }}>{t.memWorkspaceTitle}</div>
      {workspaceMem ? (
        <>
          <div className="modal-hint">{workspaceMem.dir_name}</div>
          <pre className="memory-preview">{workspaceMem.content || t.memEmpty}</pre>
        </>
      ) : (
        <div className="modal-hint">{t.memWorkspaceNone}</div>
      )}
    </div>
  );
}
