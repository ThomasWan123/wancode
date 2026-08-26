/** Modern formats accepted by the Work document picker.
 *
 * Legacy binary Office containers (.doc/.xls/.ppt) intentionally stay out of
 * this list until a sandboxed conversion path exists.
 */
export const WORK_DOCUMENT_EXTENSIONS = [
  "pdf",
  "docx",
  "xlsx",
  "pptx",
  "png",
  "jpg",
  "jpeg",
  "webp",
] as const;

const WORK_IMAGE_KINDS = new Set(["png", "jpeg", "jpg", "webp"]);

export function isWorkImageKind(kind: string): boolean {
  return WORK_IMAGE_KINDS.has(kind.toLowerCase());
}
