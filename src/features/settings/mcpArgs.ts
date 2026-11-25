/* MCP 参数行的切分。
   此前是 `args.trim().split(/\s+/)`：带空格的路径会被拆成多个参数，服务器
   直接起不来——而 Windows 上 "Program Files"、带空格的项目目录到处都是。
   现在双引号内的空白不再切分。引号内的每个字符按原样保留：Windows 路径里的
   反斜杠不是转义符，`"D:\proj\x"` 就是一个反斜杠照留的参数。 */

/** 把一行参数拆成 argv。双引号外的空白分隔参数；双引号内的内容原样保留，
 * 所以 `"D:\My Projects\x"` 是一个参数。没有闭合的引号延续到行尾。 */
export function parseMcpArgs(text: string): string[] {
  const args: string[] = [];
  let current = "";
  // open：当前已经开了一个参数（可能是 `""` 这种空参数，所以不能只看 current）
  let open = false;
  let quoted = false;
  for (const ch of text) {
    if (quoted) {
      if (ch === '"') quoted = false;
      else current += ch;
    } else if (ch === '"') {
      quoted = true;
      open = true;
    } else if (/\s/.test(ch)) {
      if (open) args.push(current);
      current = "";
      open = false;
    } else {
      current += ch;
      open = true;
    }
  }
  if (open) args.push(current);
  return args;
}
