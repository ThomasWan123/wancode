/* v0.14 工作台：聊天右侧的第二栏（单栏为回退布局，关掉即回原样）。
   首个视图 = Diff 一级视图：变更文件列表 + 逐文件 unified diff。
   红线：
   - 数据全部走引擎 git/*（ext_call 层已强制显式 gitRoot，#83 通道）；
   - 引擎响应是 {result:{files:[...]}} 信封，先判 error 再取 result；
   - 超大 patch 前端截断显示（引擎侧传 maxPatchBytes 会把整个请求打死）。 */
import { IconX } from "../../icons";

const PATCH_RENDER_LIMIT = 4000; // 行数上限：再大就只显示头部 + 提示

/** unified diff 文本 → 着色行。不做语法高亮，只分 +/-/@@/其它。 */
function PatchView({ patch, annotations }: { patch: string; annotations?: Map<number, any[]> }) {
  const lines = patch.split("\n");
  const truncated = lines.length > PATCH_RENDER_LIMIT;
  const shown = truncated ? lines.slice(0, PATCH_RENDER_LIMIT) : lines;
  // v0.15-4 行级评论：按 @@ 头跟踪新文件行号，命中 annotations 的行下方
  // 内联渲染 findings（行号对不上时评论仍在 Review 标签，不会丢）。
  let newLine = 0;
  let inHunk = false; // 哨兵：hunk 起始行可为 +1（newLine=0），不能拿 newLine>0 当"在 hunk 内"用
  return (
    <pre className="wb-patch">
      {shown.map((l, i) => {
        let cls = "";
        let lineNo: number | null = null;
        if (l.startsWith("@@")) {
          cls = "hunk";
          const m = /[+](\d+)/.exec(l);
          if (m) newLine = parseInt(m[1], 10) - 1;
          inHunk = true;
        } else if (l.startsWith("+") && !l.startsWith("+++")) {
          cls = "add";
          newLine += 1;
          lineNo = newLine;
        } else if (l.startsWith("-") && !l.startsWith("---")) {
          cls = "del";
        } else if (inHunk && !l.startsWith("\\")) {
          // 上下文行推进新文件行号；diff 头部行（inHunk 前）不计。
          // 守卫必须用 inHunk 而非 newLine>0：hunk 起始 +1 时 newLine=0，
          // 顶部上下文行会漏计导致整段错位（二轮自审抓到的 off-by-N）。
          newLine += 1;
          lineNo = newLine;
        }
        const notes = lineNo != null ? annotations?.get(lineNo) : undefined;
        return (
          <div key={i}>
            <div className={`wb-line ${cls}`}>{l || " "}</div>
            {notes?.map((f: any, j: number) => (
              <div key={j} className={`wb-inline-note ${f.severity ?? "info"}`}>
                <span className={`wb-sev ${f.severity ?? "info"}`}>
                  {f.severity === "error" ? "✕" : f.severity === "warn" ? "!" : "i"}
                </span>
                {f.comment}
              </div>
            ))}
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

/** 预览标签：iframe 嵌本地 dev server（CSP frame-src 已放行 localhost）。
    只认 http://localhost / http://127.0.0.1——预览面板不是通用浏览器。 */
function PreviewTab(props: Record<string, any>) {
  const { previewUrl, setPreviewUrl, previewLive, setPreviewLive, t } = props;
  const isLocal = (u: string) => /^http:\/\/(localhost|127\.0\.0\.1)(:\d+)?(\/|$)/.test(u);
  const go = () => {
    const u = previewUrl.trim();
    if (!isLocal(u)) return;
    localStorage.setItem("wancode-preview-url", u);
    // 相同 URL 重按 = 强制重载：先卸载 iframe 再挂
    setPreviewLive(null);
    setTimeout(() => setPreviewLive(u), 30);
  };
  return (
    <div className="wb-body wb-preview">
      <div className="wb-review-bar">
        <input
          className="session-search wb-preview-url"
          value={previewUrl}
          placeholder="http://localhost:5173"
          onChange={(e) => setPreviewUrl(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Enter" && go()}
        />
        <button onClick={go} disabled={!isLocal(previewUrl.trim())}>
          {previewLive ? t.previewReload : t.previewOpen}
        </button>
      </div>
      {!isLocal(previewUrl.trim()) && previewUrl.trim() !== "" && (
        <div className="sidebar-empty">{t.previewLocalOnly}</div>
      )}
      {previewLive ? (
        <iframe className="wb-preview-frame" src={previewLive} title="preview" />
      ) : (
        <div className="sidebar-empty">{t.previewHint}</div>
      )}
    </div>
  );
}

/** Review 标签：一键只读审查未提交改动，findings 按文件分组渲染。 */
function ReviewTab(props: Record<string, any>) {
  const { reviewResult, reviewLoading, runReview, fixFindings, t } = props;
  const findings: any[] = Array.isArray(reviewResult?.findings) ? reviewResult.findings : [];
  const byFile = new Map<string, any[]>();
  for (const f of findings) {
    const k = f?.file ?? "?";
    if (!byFile.has(k)) byFile.set(k, []);
    byFile.get(k)!.push(f);
  }
  return (
    <div className="wb-body">
      <div className="wb-review-bar">
        <button disabled={reviewLoading} onClick={runReview}>
          {reviewLoading ? t.reviewRunning : t.reviewRun}
        </button>
        {findings.length > 0 && !reviewLoading && (
          <button className="ghost" onClick={() => fixFindings(findings)}>
            {t.reviewFixAll(findings.length)}
          </button>
        )}
        {reviewResult && !reviewLoading && (
          <span className="wb-stats">
            {t.reviewSummary(reviewResult.reviewedFiles ?? 0, findings.length)}
            {(reviewResult.skippedFiles?.length ?? 0) > 0 &&
              ` · ${t.reviewSkipped(reviewResult.skippedFiles.length)}`}
          </span>
        )}
      </div>
      {reviewLoading && <div className="sidebar-empty">{t.reviewHint}</div>}
      {!reviewLoading && reviewResult && findings.length === 0 && reviewResult.findings !== null && (
        <div className="sidebar-empty">✅ {t.reviewClean}</div>
      )}
      {!reviewLoading && reviewResult?.findings === null && (
        // 解析失败：退回显示原文，绝不假装"没有问题"
        <pre className="wb-patch wb-review-raw">{reviewResult.raw}</pre>
      )}
      {[...byFile.entries()].map(([file, list]) => (
        <div key={file} className="wb-file">
          <div className="wb-file-head wb-review-file" title={file}>
            <span className="wb-file-path">{file}</span>
            <span className="wb-stats">{list.length}</span>
          </div>
          {list.map((f, i) => (
            <div key={i} className={`wb-finding ${f.severity ?? "info"}`}>
              <span className={`wb-sev ${f.severity ?? "info"}`}>
                {f.severity === "error" ? "✕" : f.severity === "warn" ? "!" : "i"}
              </span>
              <span className="wb-finding-text">
                {f.line != null && <span className="wb-lineno">L{f.line}</span>}
                {f.comment}
              </span>
              <button
                className="wb-fix-btn"
                title={t.reviewFixOne}
                aria-label={t.reviewFixOne}
                onClick={() => fixFindings([f])}
              >
                🔧
              </button>
            </div>
          ))}
        </div>
      ))}
    </div>
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
  // 审查 findings 按 文件→行号 索引，Diff 视图内联行级评论用
  const reviewFindings: any[] = Array.isArray(props.reviewResult?.findings)
    ? props.reviewResult.findings
    : [];
  const annotationsFor = (path: string): Map<number, any[]> | undefined => {
    const norm = (p: string) => (p ?? "").split("\\").join("/").toLowerCase();
    const hits = reviewFindings.filter((f) => f.line != null && norm(f.file) === norm(path));
    if (!hits.length) return undefined;
    const m = new Map<number, any[]>();
    for (const f of hits) {
      const arr = m.get(f.line) ?? [];
      arr.push(f);
      m.set(f.line, arr);
    }
    return m;
  };
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
          <button
            className={`wb-tab ${wbTab === "review" ? "active" : ""}`}
            onClick={() => setWbTab("review")}
          >
            {t.wbReviewTitle}
          </button>
          <button
            className={`wb-tab ${wbTab === "preview" ? "active" : ""}`}
            onClick={() => setWbTab("preview")}
          >
            {t.wbPreviewTitle}
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
      {wbTab === "review" && <ReviewTab {...props} />}
      {wbTab === "preview" && <PreviewTab {...props} />}
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
                    <PatchView patch={f.patch} annotations={annotationsFor(f.path)} />
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
