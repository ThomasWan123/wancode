/* v0.14 工作台：聊天右侧的第二栏（单栏为回退布局，关掉即回原样）。
   首个视图 = Diff 一级视图：变更文件列表 + 逐文件 unified diff。
   红线：
   - 数据全部走引擎 git/*（ext_call 层已强制显式 gitRoot，#83 通道）；
   - 引擎响应是 {result:{files:[...]}} 信封，先判 error 再取 result；
   - 超大 patch 前端截断显示（引擎侧传 maxPatchBytes 会把整个请求打死）。 */
import { IconX } from "../../icons";

const PATCH_RENDER_LIMIT = 4000; // 行数上限：再大就只显示头部 + 提示

/** unified diff 文本 → 着色行。不做语法高亮，只分 +/-/@@/其它。 */
function PatchView({ patch }: { patch: string }) {
  const lines = patch.split("\n");
  const truncated = lines.length > PATCH_RENDER_LIMIT;
  const shown = truncated ? lines.slice(0, PATCH_RENDER_LIMIT) : lines;
  return (
    <pre className="wb-patch">
      {shown.map((l, i) => {
        const cls = l.startsWith("+") && !l.startsWith("+++")
          ? "add"
          : l.startsWith("-") && !l.startsWith("---")
            ? "del"
            : l.startsWith("@@")
              ? "hunk"
              : "";
        return (
          <div key={i} className={`wb-line ${cls}`}>
            {l || " "}
          </div>
        );
      })}
      {truncated && (
        <div className="wb-line hunk">… {lines.length - PATCH_RENDER_LIMIT} more lines (truncated)</div>
      )}
    </pre>
  );
}

/** 文件查看标签：过滤输入 + 文件列表 + 只读内容（行号）。 */
function FileTab(props: Record<string, any>) {
  const { fileList, wbFilePath, wbFileText, wbFileLoading, openWbFile, wbFileFilter, setWbFileFilter, t } = props;
  const hits: string[] = wbFileFilter
    ? (fileList as string[]).filter((p) => {
        // 子序列匹配，与 @ 联想同语义
        const q = wbFileFilter.toLowerCase();
        const s = p.toLowerCase();
        let i = 0;
        for (const ch of s) {
          if (ch === q[i]) i++;
          if (i === q.length) return true;
        }
        return false;
      })
    : fileList;
  const lines = (wbFileText ?? "").split("\n");
  const MAX = 3000;
  const shown = lines.length > MAX ? lines.slice(0, MAX) : lines;
  return (
    <>
      <input
        className="session-search wb-file-filter"
        value={wbFileFilter}
        placeholder={t.wbFileFilter}
        onChange={(e) => setWbFileFilter(e.currentTarget.value)}
      />
      {!wbFilePath ? (
        <div className="wb-body">
          {hits.slice(0, 400).map((p) => (
            <div key={p} className="wb-file-row" title={p} onClick={() => openWbFile(p)}>
              {p}
            </div>
          ))}
          {hits.length === 0 && <div className="sidebar-empty">{t.grepNoHits}</div>}
        </div>
      ) : (
        <div className="wb-body">
          <div className="wb-file-row wb-file-back" onClick={() => openWbFile(null)}>
            ← {wbFilePath}
          </div>
          {wbFileLoading && <div className="sidebar-empty">{t.loading}</div>}
          {!wbFileLoading && wbFileText === null && <div className="sidebar-empty">{t.wbNoPatch}</div>}
          {!wbFileLoading && wbFileText !== null && (
            <pre className="wb-patch wb-file-view">
              {shown.map((l: string, i: number) => (
                <div key={i} className="wb-line">
                  <span className="wb-lineno">{i + 1}</span>
                  {l || " "}
                </div>
              ))}
              {lines.length > MAX && (
                <div className="wb-line hunk">… {lines.length - MAX} more lines (truncated)</div>
              )}
            </pre>
          )}
        </div>
      )}
    </>
  );
}

export function Workbench(props: Record<string, any>) {
  const {
    showWorkbench,
    setShowWorkbench,
    wbTab,
    setWbTab,
    wbFiles, // GitFileChange[] | null（null=未加载/非仓库）
    wbLoading,
    wbOpenPaths, // Set<string> 展开中的文件
    setWbOpenPaths,
    refreshWorkbench,
    gitOp, // (cmd, args) => Promise —— 复用 App 层封装（操作后自动 refreshGit）
    t,
  } = props;
  if (!showWorkbench) return null;

  const files: any[] = wbFiles ?? [];
  const totalAdd = files.reduce((s, f) => s + (f.additions ?? 0), 0);
  const totalDel = files.reduce((s, f) => s + (f.deletions ?? 0), 0);
  const toggle = (p: string) =>
    setWbOpenPaths((prev: Set<string>) => {
      const next = new Set(prev);
      if (next.has(p)) next.delete(p);
      else next.add(p);
      return next;
    });

  return (
    <aside className="workbench">
      <div className="wb-head">
        <span className="wb-title">
          <button
            className={`wb-tab ${wbTab === "diff" ? "active" : ""}`}
            onClick={() => setWbTab("diff")}
          >
            {t.wbDiffTitle}
          </button>
          <button
            className={`wb-tab ${wbTab === "file" ? "active" : ""}`}
            onClick={() => setWbTab("file")}
          >
            {t.wbFileTitle}
          </button>
          {wbTab === "diff" && files.length > 0 && (
            <span className="wb-stats">
              {files.length} · <em className="add">+{totalAdd}</em> <em className="del">−{totalDel}</em>
            </span>
          )}
        </span>
        {wbTab === "diff" && (
          <button className="icon-btn" title={t.refresh} onClick={refreshWorkbench}>
            ⟳
          </button>
        )}
        <button className="icon-btn" title={t.close} onClick={() => setShowWorkbench(false)}>
          <IconX size={15} />
        </button>
      </div>
      {wbTab === "file" && <FileTab {...props} />}
      {wbTab === "diff" && (
      <div className="wb-body">
        {wbLoading && <div className="sidebar-empty">{t.loading}</div>}
        {!wbLoading && wbFiles === null && <div className="sidebar-empty">{t.gitNotRepo}</div>}
        {!wbLoading && wbFiles !== null && files.length === 0 && (
          <div className="sidebar-empty">{t.gitClean}</div>
        )}
        {files.map((f) => {
          const open = wbOpenPaths.has(f.path);
          return (
            <div key={f.path} className="wb-file">
              <div className="wb-file-head" onClick={() => toggle(f.path)} title={f.path}>
                <span className={`wb-chev ${open ? "open" : ""}`}>▸</span>
                <span className={`wb-badge ${f.type}`}>{(f.type ?? "?")[0].toUpperCase()}</span>
                <span className="wb-file-path">{f.path}</span>
                {f.staged === true && <span className="wb-staged">{t.gitStagedBadge}</span>}
                <span className="wb-file-stats">
                  <em className="add">+{f.additions ?? 0}</em> <em className="del">−{f.deletions ?? 0}</em>
                </span>
              </div>
              {open && (
                <>
                  <div className="wb-file-ops">
                    {f.staged === true ? (
                      <button onClick={() => gitOp("git_unstage", { paths: [f.path] }).then(refreshWorkbench)}>
                        {t.gitUnstage}
                      </button>
                    ) : (
                      <button onClick={() => gitOp("git_stage", { paths: [f.path] }).then(refreshWorkbench)}>
                        {t.gitStage}
                      </button>
                    )}
                    <button
                      className="deny"
                      onClick={() => {
                        if (!window.confirm(t.gitDiscardConfirm(f.path))) return;
                        gitOp("git_discard", { paths: [f.path] }).then(refreshWorkbench);
                      }}
                    >
                      {t.gitDiscard}
                    </button>
                  </div>
                  {f.patch ? (
                    <PatchView patch={f.patch} />
                  ) : (
                    <div className="sidebar-empty">{t.wbNoPatch}</div>
                  )}
                </>
              )}
            </div>
          );
        })}
      </div>
      )}
    </aside>
  );
}
