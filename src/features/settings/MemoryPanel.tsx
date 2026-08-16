/* C3（v0.20）：项目记忆设置面。状态自包含——设置页是低频面，不再给
   App 的 prop 袋加码。开关/flush/rewrite 全部直打 Tauri 命令边界。 */
import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export function MemoryPanel(props: { sessionId: string; surface: string; workspace: string; t: any }) {
  const { sessionId, surface, workspace, t } = props;
  const [enabled, setEnabled] = useState(false);
  const [globalMem, setGlobalMem] = useState("");
  const [workspaceMem, setWorkspaceMem] = useState<{ dir_name: string; content: string } | null>(null);
  const [rawNote, setRawNote] = useState("");
  const [rewritten, setRewritten] = useState("");
  const [busy, setBusy] = useState<"" | "flush" | "rewrite" | "append">("");
  const [msg, setMsg] = useState("");

  const refresh = useCallback(async () => {
    try {
      setEnabled(await invoke<boolean>("memory_config_get"));
    } catch {
      /* 配置不可读时保持关——设置页不因此报错 */
    }
    try {
      setGlobalMem(await invoke<string>("memory_read_global"));
    } catch {
      /* 同上 */
    }
    if (workspace) {
      try {
        setWorkspaceMem(await invoke<any>("memory_read_workspace", { workspace }));
      } catch {
        /* best-effort 发现失败 = 无工作区记忆 */
      }
    }
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

  const rewrite = async () => {
    if (!rawNote.trim()) return;
    setBusy("rewrite");
    setMsg("");
    try {
      const r = await invoke<any>("memory_rewrite", { rawText: rawNote.trim() });
      const text = typeof r?.rewritten === "string" ? r.rewritten : "";
      setRewritten(text);
      if (!text) setMsg(t.memRewriteEmpty);
    } catch (e) {
      setMsg(String(e));
    } finally {
      setBusy("");
    }
  };

  const append = async () => {
    if (!rewritten.trim()) return;
    setBusy("append");
    setMsg("");
    try {
      await invoke("memory_append_global", { text: rewritten.trim() });
      setRewritten("");
      setRawNote("");
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

      <div className="modal-label" style={{ marginTop: 16 }}>{t.memRewriteLabel}</div>
      <textarea
        rows={3}
        style={{ width: "100%" }}
        placeholder={t.memRewritePlaceholder}
        value={rawNote}
        onChange={(e) => setRawNote(e.currentTarget.value)}
      />
      <button disabled={!sessionId || !rawNote.trim() || busy !== ""} onClick={() => void rewrite()}>
        {busy === "rewrite" ? t.memRewriteBusy : t.memRewriteBtn}
      </button>
      {rewritten && (
        <>
          <pre className="memory-preview">{rewritten}</pre>
          <button disabled={busy !== ""} onClick={() => void append()}>
            {busy === "append" ? t.memAppendBusy : t.memAppend}
          </button>
        </>
      )}

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
