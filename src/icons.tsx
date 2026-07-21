// 细线 SVG 图标（Lucide 风格，1.5 描边）—— 替代 emoji，对标 Claude Code
import type { CSSProperties } from "react";

type P = { size?: number; className?: string; style?: CSSProperties };
const base = (size: number): any => ({
  width: size,
  height: size,
  viewBox: "0 0 24 24",
  fill: "none",
  stroke: "currentColor",
  strokeWidth: 1.6,
  strokeLinecap: "round",
  strokeLinejoin: "round",
});

export const IconFolder = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
);
export const IconSettings = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" /></svg>
);
export const IconSun = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><circle cx="12" cy="12" r="4" /><path d="M12 2v2M12 20v2M4.9 4.9l1.4 1.4M17.7 17.7l1.4 1.4M2 12h2M20 12h2M4.9 19.1l1.4-1.4M17.7 6.3l1.4-1.4" /></svg>
);
export const IconMoon = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M21 12.8A9 9 0 1 1 11.2 3a7 7 0 0 0 9.8 9.8z" /></svg>
);
export const IconRewind = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M3 12a9 9 0 1 0 3-6.7L3 8" /><path d="M3 3v5h5" /></svg>
);
export const IconGitBranch = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><line x1="6" y1="3" x2="6" y2="15" /><circle cx="18" cy="6" r="3" /><circle cx="6" cy="18" r="3" /><path d="M18 9a9 9 0 0 1-9 9" /></svg>
);
export const IconClipboard = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><rect x="8" y="2" width="8" height="4" rx="1" /><path d="M16 4h2a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2h2" /><path d="M9 12h6M9 16h4" /></svg>
);
export const IconTerminal = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M4 17l6-5-6-5M12 19h8" /></svg>
);
export const IconArrowUp = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M12 19V5M5 12l7-7 7 7" /></svg>
);
export const IconStop = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><rect x="6" y="6" width="12" height="12" rx="2" /></svg>
);
export const IconPlus = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M12 5v14M5 12h14" /></svg>
);
export const IconSearch = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><circle cx="11" cy="11" r="7" /><path d="m21 21-4.3-4.3" /></svg>
);
export const IconX = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M18 6 6 18M6 6l12 12" /></svg>
);
export const IconPencil = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4z" /></svg>
);
export const IconTrash = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M3 6h18M8 6V4a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v2M19 6l-1 14a2 2 0 0 1-2 2H8a2 2 0 0 1-2-2L5 6" /></svg>
);
export const IconWrench = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M14.7 6.3a4 4 0 0 0-5.4 5.4L3 18l3 3 6.3-6.3a4 4 0 0 0 5.4-5.4l-2.6 2.6-2.4-2.4z" /></svg>
);
export const IconFile = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" /><path d="M14 2v6h6" /></svg>
);
export const IconFolderClosed = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" /></svg>
);
export const IconCheck = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M20 6 9 17l-5-5" /></svg>
);
export const IconCopy = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h10" /></svg>
);
export const IconShield = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="M12 3l7 3v5c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z" /></svg>
);
export const IconChevron = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><path d="m6 9 6 6 6-6" /></svg>
);
export const IconColumns = ({ size = 16, ...r }: P) => (
  <svg {...base(size)} {...r}><rect x="3" y="4" width="18" height="16" rx="2" /><path d="M14 4v16" /></svg>
);
