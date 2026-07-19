// 交互式 PTY 终端。
//
// 引擎那边 PTY 是 agent 级的、比会话活得久：terminalId 由 pty/create 返回，
// 之后所有输入/输出/resize 都围着它转，输出按 16ms 批量推、带 256KiB 环形
// 缓冲，重连时整段重放。
//
// 全程按 **字节** 走，不在中间解码成文本：PTY 输出会在任意字节边界切断，
// 按 UTF-8 解码必然切坏多字节字符（中文）和 ANSI 转义序列。xterm 的
// write() 接受 Uint8Array 并自己处理跨块的半个字符。
import { useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";

const b64ToBytes = (b64: string) => {
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
};

// 会话 → PTY id 映射（模块级，跨挂载存活）：切走标签/切会话回来时
// 用 pty/load 重放整段缓冲重连，而不是杀掉重建。
const ptyBySession = new Map<string, string>();

const bytesToB64 = (bytes: Uint8Array) => {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return btoa(s);
};

export default function PtyTerm({
  dark,
  sessionKey,
  onError,
}: {
  dark: boolean;
  sessionKey: string;
  onError: (e: string) => void;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  const termRef = useRef<Terminal | null>(null);
  const idRef = useRef<string>("");

  useEffect(() => {
    if (!hostRef.current) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;

    const term = new Terminal({
      fontSize: 12.5,
      fontFamily: 'var(--font-mono), "Cascadia Mono", Consolas, monospace',
      cursorBlink: true,
      convertEol: false,
      theme: dark
        ? { background: "#171717", foreground: "#ececec", cursor: "#ececec" }
        : { background: "#ffffff", foreground: "#1a1a1a", cursor: "#1a1a1a" },
    });
    const fit = new FitAddon();
    term.loadAddon(fit);
    term.open(hostRef.current);
    fit.fit();
    termRef.current = term;

    (async () => {
      try {
        // 先订阅再创建：反过来的话，shell 启动横幅可能在监听器装好之前就推来了
        unlisten = await listen<any>("agent://ext", (e) => {
          if (e.payload?.method !== "x.ai/terminal/pty/notification") return;
          const p = e.payload.params ?? {};
          if (!idRef.current || p.terminalId !== idRef.current) return;
          if (p.type === "output" && typeof p.data === "string") {
            term.write(b64ToBytes(p.data));
          } else if (p.type === "exit") {
            term.write(`\r\n\x1b[2m[exited${
              p.exitCode != null ? ` (${p.exitCode})` : ""
            }]\x1b[0m\r\n`);
            idRef.current = "";
          }
        });
        if (disposed) return;

        // 已有存活 PTY → pty/load 重连（引擎重放 256KiB 环形缓冲），
        // 否则新建。load 失败（进程已退出被回收等）退回新建。
        let id = ptyBySession.get(sessionKey) ?? "";
        if (id) {
          try {
            await invoke("pty_load", { terminalId: id });
          } catch {
            id = "";
          }
        }
        if (!id) {
          id = await invoke<string>("pty_create", {
            rows: term.rows,
            cols: term.cols,
          });
        }
        if (disposed) return;
        ptyBySession.set(sessionKey, id);
        idRef.current = id;

        term.onData((s) => {
          if (!idRef.current) return;
          const bytes = new TextEncoder().encode(s);
          invoke("pty_input", {
            terminalId: idRef.current,
            data: bytesToB64(bytes),
          }).catch((err) => onError(String(err)));
        });
      } catch (err) {
        onError(String(err));
      }
    })();

    const ro = new ResizeObserver(() => {
      try {
        fit.fit();
      } catch {
        /* 面板收起时容器是 0 尺寸，fit 会抛，忽略 */
      }
      if (idRef.current) {
        invoke("pty_resize", {
          terminalId: idRef.current,
          rows: term.rows,
          cols: term.cols,
        }).catch(() => {});
      }
    });
    ro.observe(hostRef.current);

    return () => {
      disposed = true;
      ro.disconnect();
      unlisten?.();
      // 不再 unmount 即 kill：id 存在模块级映射里，下次挂载 pty/load 重连。
      // 引擎在 agent 断开时统一回收（close_all）。
      term.dispose();
    };
    // dark 变化只改主题，不该重建终端（会丢掉整个 shell 会话）
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!termRef.current) return;
    termRef.current.options.theme = dark
      ? { background: "#171717", foreground: "#ececec", cursor: "#ececec" }
      : { background: "#ffffff", foreground: "#1a1a1a", cursor: "#1a1a1a" };
  }, [dark]);

  return <div className="pty-host" ref={hostRef} />;
}
