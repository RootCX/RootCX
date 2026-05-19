/**
 * Extracts the RFC 2822 Message-ID header from a raw email buffer.
 * Handles folded continuation lines per RFC 5322 §2.2.3.
 * Returns empty string when absent.
 */
export function extractMessageIdFromBuffer(buf: Buffer): string {
  const text = buf.toString("utf-8");
  const sep = text.indexOf("\r\n\r\n");
  const headers = sep === -1 ? text : text.slice(0, sep);
  const unfolded = headers.replace(/\r\n([ \t])/g, "$1");
  const m = unfolded.match(/^Message-ID:\s*(.+)$/im);
  return m?.[1]?.trim() ?? "";
}
