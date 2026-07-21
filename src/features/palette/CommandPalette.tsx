/* v0.14 命令面板（Ctrl+K）：聚合应用级动作，模糊过滤 + 键盘导航。
   动作列表由 App 层组装传入（带 disabled 状态），这里只管过滤与选择。
   斜杠命令不进这里——输入框的 / 联想已有专门通路，两处重复会打架。 */
import { useEffect, useRef, useState } from "react";

export type PaletteAction = {
  id: string;
  label: string;
  hint?: string; // 快捷键提示
  disabled?: boolean;
  run: () => void;
};

/** 子序列模糊匹配（大小写不敏感）——与侧栏 @ 文件联想同一语义。 */
function fuzzy(q: string, s: string): boolean {
  if (!q) return true;
  const lq = q.toLowerCase();
  const ls = s.toLowerCase();
  let i = 0;
  for (const ch of ls) {
    if (ch === lq[i]) i++;
    if (i === lq.length) return true;
  }
  return false;
}

export function CommandPalette({
  actions,
  onClose,
  t,
}: {
  actions: PaletteAction[];
  onClose: () => void;
  t: Record<string, any>;
}) {
  const [query, setQuery] = useState("");
  const [sel, setSel] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const hits = actions.filter((a) => !a.disabled && fuzzy(query, a.label));
  const clamped = Math.min(sel, Math.max(0, hits.length - 1));

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const run = (a: PaletteAction) => {
    onClose();
    a.run();
  };

  return (
    <div className="modal-mask palette-mask" onClick={onClose}>
      <div className="palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="palette-input"
          value={query}
          placeholder={t.paletteHint}
          onChange={(e) => {
            setQuery(e.currentTarget.value);
            setSel(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setSel((v) => (v + 1) % Math.max(1, hits.length));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setSel((v) => (v - 1 + Math.max(1, hits.length)) % Math.max(1, hits.length));
            } else if (e.key === "Enter") {
              e.preventDefault();
              if (hits[clamped]) run(hits[clamped]);
            } else if (e.key === "Escape") {
              e.preventDefault();
              onClose();
            }
          }}
        />
        <div className="palette-list">
          {hits.length === 0 && <div className="sidebar-empty">{t.paletteNoHits}</div>}
          {hits.map((a, i) => (
            <div
              key={a.id}
              className={`palette-item ${i === clamped ? "active" : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                run(a);
              }}
              onMouseEnter={() => setSel(i)}
            >
              <span className="palette-label">{a.label}</span>
              {a.hint && <span className="palette-key">{a.hint}</span>}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
