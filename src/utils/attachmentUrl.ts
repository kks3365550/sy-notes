import { convertFileSrc, isTauri } from "@tauri-apps/api/core";

/** Custom URI scheme registered in src-tauri/src/lib.rs (attachment::URI_SCHEME). */
export const ATTACHMENT_SCHEME = "attachment";

/** Prefix of attachment URLs stored in document JSON (kept stable for the orphan scanner). */
export const STORED_ATTACHMENT_PREFIX = "attachments/";

const STORED_ATTACHMENT_RE = /^attachments\/([A-Za-z0-9_-]+\.[A-Za-z0-9]+)$/;

/**
 * Returns the bare file name if `url` is a stored attachment URL (`attachments/<file>`),
 * otherwise null. Anything with extra path segments, `..`, encoding etc. is not matched.
 */
export function parseStoredAttachmentUrl(url: string): string | null {
  const m = STORED_ATTACHMENT_RE.exec(url);
  return m ? m[1] : null;
}

/**
 * Maps a stored attachment URL to the URL the webview can load. Other URLs
 * (http(s), data:, blob:) are returned unchanged.
 *
 * `convert` defaults to Tauri's convertFileSrc, which yields
 * `attachment://localhost/<file>` on macOS/Linux and
 * `http://attachment.localhost/<file>` on Windows/Android.
 */
export function toAttachmentDisplayUrl(
  url: string,
  convert: (path: string, protocol: string) => string = convertFileSrc
): string {
  const file = parseStoredAttachmentUrl(url);
  return file ? convert(file, ATTACHMENT_SCHEME) : url;
}

/** BlockNote `resolveFileUrl` handler: display-only mapping, the document keeps `attachments/<file>`. */
export async function resolveAttachmentUrl(url: string): Promise<string> {
  if (!isTauri()) return url;
  return toAttachmentDisplayUrl(url);
}
