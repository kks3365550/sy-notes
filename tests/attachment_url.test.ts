import { describe, it, expect } from "vitest";
import {
  parseStoredAttachmentUrl,
  toAttachmentDisplayUrl,
  ATTACHMENT_SCHEME,
} from "../src/utils/attachmentUrl";

// Mirrors Tauri's convertFileSrc for both platform families.
const unixConvert = (p: string, proto: string) => `${proto}://localhost/${encodeURIComponent(p)}`;
const winConvert = (p: string, proto: string) => `http://${proto}.localhost/${encodeURIComponent(p)}`;

describe("attachment URL mapping", () => {
  const file = "3f2b8c1e-1a2b-4c3d-9e8f-0123456789ab.png";

  it("parses stored attachment URLs", () => {
    expect(parseStoredAttachmentUrl(`attachments/${file}`)).toBe(file);
  });

  it("rejects anything that is not a single plain file name", () => {
    for (const bad of [
      "attachments/",
      "attachments/../sy_notes.db",
      "attachments/a/b.png",
      "attachments/..%2Fsy_notes.db",
      "/attachments/x.png",
      "./attachments/x.png",
      "attachments/x",
      "https://example.com/attachments/x.png",
    ]) {
      expect(parseStoredAttachmentUrl(bad), bad).toBeNull();
    }
  });

  it("maps stored URLs to the attachment scheme", () => {
    expect(ATTACHMENT_SCHEME).toBe("attachment");
    expect(toAttachmentDisplayUrl(`attachments/${file}`, unixConvert)).toBe(`attachment://localhost/${file}`);
    expect(toAttachmentDisplayUrl(`attachments/${file}`, winConvert)).toBe(`http://attachment.localhost/${file}`);
  });

  it("leaves other URLs untouched", () => {
    for (const url of ["https://example.com/a.png", "data:image/png;base64,AAAA", "blob:http://x/1", "attachments/../x.png"]) {
      expect(toAttachmentDisplayUrl(url, unixConvert)).toBe(url);
    }
  });
});
