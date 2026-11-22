import React, {
  useEffect,
  useRef,
  useCallback,
  useImperativeHandle,
  forwardRef,
  useState,
} from "react";
import { BlockNoteEditor, PartialBlock } from "@blocknote/core";
import { useCreateBlockNote } from "@blocknote/react";
import { BlockNoteView } from "@blocknote/mantine";
import { DocumentItem } from "../types";
import { saveAttachment } from "../api";
import {
  AUTOSAVE_DELAY_MS,
  AutosaveGate,
  attachCompositionListeners,
  createAutosaveGate,
} from "../utils/autosaveGate";
import "@blocknote/core/fonts/inter.css";
import "@blocknote/mantine/style.css";

/**
 * Extracted payload when editor contents change/save
 */
export interface EditorSaveData {
  content_json: string;
  content_text: string;
}

export interface EditorProps {
  /** Complete DocumentItem passed from parent */
  document?: DocumentItem | null;
  /** Identifier of current active document (triggers clean re-init when switched) */
  docId?: string;
  /** JSON string containing array of BlockNote blocks */
  initialContent?: string | null;
  /** Read-only mode toggle */
  editable?: boolean;
  /** Color theme */
  theme?: "light" | "dark";
  /** Debounced auto-save callback (fires 500ms after typing/IME stops) */
  onSave?: ((data: EditorSaveData) => void) | ((updatedDoc: DocumentItem) => void | Promise<void>);
  /** Change callback */
  onChange?: (data: EditorSaveData) => void;
  /** Callback fired when Korean IME composition starts or ends */
  onCompositionChange?: (isComposing: boolean) => void;
  /** Save status notifier ("saved" | "saving" | "composing") */
  onStatusChange?: (status: "saved" | "saving" | "composing") => void;
  /** Ref to allow external components (e.g. AIPanel) to insert generated text */
  onInsertTextRef?: React.MutableRefObject<((text: string) => void) | null>;
  /** File upload handler for attachments */
  uploadFile?: (file: File) => Promise<string>;
  /** Optional container CSS class */
  className?: string;
  /** Show live character and block count */
  showStatusFooter?: boolean;
}

/**
 * Imperative handle methods exposed via ref
 */
export interface EditorHandle {
  flush: () => void;
  isComposing: () => boolean;
  getContent: () => EditorSaveData;
  getEditor: () => BlockNoteEditor | null;
  insertText: (text: string) => void;
  replaceBlocks: (blocks: PartialBlock[]) => void;
}

/**
 * Extracts plain text from BlockNote blocks recursively for FTS5 indexing.
 */
export function blocksToPlainText(blocks: any[]): string {
  if (!Array.isArray(blocks) || blocks.length === 0) return "";
  const lines: string[] = [];

  for (const block of blocks) {
    let line = "";
    if (Array.isArray(block.content)) {
      for (const item of block.content) {
        if (typeof item === "string") {
          line += item;
        } else if (item && typeof item === "object") {
          if (item.type === "text" && typeof item.text === "string") {
            line += item.text;
          } else if (item.type === "link") {
            if (Array.isArray(item.content)) {
              line += item.content.map((c: any) => c.text || "").join("");
            } else if (typeof item.text === "string") {
              line += item.text;
            }
          } else if (typeof item.text === "string") {
            line += item.text;
          }
        }
      }
    } else if (typeof block.content === "string") {
      line += block.content;
    }

    if (line.trim().length > 0) {
      lines.push(line);
    }

    if (Array.isArray(block.children) && block.children.length > 0) {
      const childText = blocksToPlainText(block.children);
      if (childText.trim().length > 0) {
        lines.push(childText);
      }
    }
  }

  return lines.join("\n");
}

/**
 * Parses initialContent JSON safely
 */
export function parseInitialBlocks(raw?: string | null): PartialBlock[] | undefined {
  if (!raw || typeof raw !== "string" || raw.trim() === "") return undefined;
  try {
    const parsed = JSON.parse(raw);
    if (Array.isArray(parsed) && parsed.length > 0) {
      return parsed as PartialBlock[];
    }
  } catch (err) {
    console.warn("[Editor] Failed to parse initialContent JSON:", err);
  }
  return undefined;
}

/**
 * Default file upload handler that saves attachment via Tauri backend
 */
async function defaultUploadFile(file: File): Promise<string> {
  try {
    const arrayBuffer = await file.arrayBuffer();
    const bytes = new Uint8Array(arrayBuffer);
    let binary = "";
    for (let i = 0; i < bytes.byteLength; i++) {
      binary += String.fromCharCode(bytes[i]);
    }
    const base64 = btoa(binary);
    return await saveAttachment(file.name, base64);
  } catch (err) {
    console.warn("[Editor] uploadFile error, fallback to ObjectURL:", err);
    return URL.createObjectURL(file);
  }
}

export const Editor = forwardRef<EditorHandle, EditorProps>(function Editor(
  {
    document: docProp,
    docId: explicitDocId,
    initialContent: explicitInitialContent,
    editable = true,
    theme = "light",
    onSave,
    onChange,
    onCompositionChange,
    onStatusChange,
    onInsertTextRef,
    uploadFile,
    className = "",
    showStatusFooter = false,
  },
  ref
) {
  const activeDocId = docProp ? docProp.id : explicitDocId;
  const activeInitialContent = docProp ? docProp.content : explicitInitialContent;

  // 1. Refs
  const lastSavedJsonRef = useRef<string>(activeInitialContent || "");
  const containerRef = useRef<HTMLDivElement>(null);
  const docPropRef = useRef(docProp);
  docPropRef.current = docProp;

  // Keep latest callbacks in refs to avoid re-binding
  const onSaveRef = useRef(onSave);
  onSaveRef.current = onSave;
  const onChangeRef = useRef(onChange);
  onChangeRef.current = onChange;
  const onCompositionChangeRef = useRef(onCompositionChange);
  onCompositionChangeRef.current = onCompositionChange;
  const onStatusChangeRef = useRef(onStatusChange);
  onStatusChangeRef.current = onStatusChange;

  // Track status for footer display
  const [composingStatus, setComposingStatus] = useState<boolean>(false);
  const [stats, setStats] = useState<{ charCount: number; blockCount: number }>({
    charCount: 0,
    blockCount: 0,
  });

  // 2. Initialize BlockNote Editor instance (re-creates cleanly per activeDocId)
  const initialBlocks = parseInitialBlocks(activeInitialContent);
  const editor = useCreateBlockNote(
    {
      initialContent: initialBlocks,
      uploadFile: uploadFile || defaultUploadFile,
    },
    [activeDocId]
  );
  const editorRef = useRef(editor);
  editorRef.current = editor;

  // 3. Save the current content (called by the autosave gate when it is safe)
  const saveNow = useCallback(() => {
    const currentDoc = editor.document;
    const content_json = JSON.stringify(currentDoc);
    const content_text = blocksToPlainText(currentDoc);

    // Skip if unchanged
    if (content_json === lastSavedJsonRef.current) {
      onStatusChangeRef.current?.("saved");
      return;
    }
    lastSavedJsonRef.current = content_json;

    const payload: EditorSaveData = { content_json, content_text };

    onStatusChangeRef.current?.("saving");

    let parentOwnsStatus = false;
    if (onSaveRef.current) {
      if (docPropRef.current) {
        // Keep `updated_at` exactly as received: it is the optimistic-lock version
        // the backend expects. The parent decides the real version at save time.
        const updatedDoc: DocumentItem = {
          ...docPropRef.current,
          content: content_json,
          content_text,
        };
        (onSaveRef.current as any)(updatedDoc);
        // The parent reports "saved" / errors once the backend actually commits.
        parentOwnsStatus = true;
      } else {
        (onSaveRef.current as any)(payload);
      }
    }

    onChangeRef.current?.(payload);
    if (!parentOwnsStatus) onStatusChangeRef.current?.("saved");

    if (showStatusFooter) {
      setStats({
        charCount: content_text.length,
        blockCount: currentDoc.length,
      });
    }
  }, [editor, showStatusFooter]);
  const saveNowRef = useRef(saveNow);
  saveNowRef.current = saveNow;

  // 4. IME-aware autosave gate (logic lives in utils/autosaveGate.ts, unit-tested)
  const gateRef = useRef<AutosaveGate | null>(null);
  if (gateRef.current === null) {
    gateRef.current = createAutosaveGate({
      delayMs: AUTOSAVE_DELAY_MS,
      onFlush: () => saveNowRef.current(),
      // ProseMirror's own composing flag, in addition to composition events
      isExternallyComposing: () => Boolean((editorRef.current.prosemirrorView as any)?.composing),
      onPending: () => onStatusChangeRef.current?.("saving"),
      onCompositionChange: (composing) => {
        setComposingStatus(composing);
        if (composing) {
          onStatusChangeRef.current?.("composing");
        } else if (!gateRef.current?.isDirty()) {
          onStatusChangeRef.current?.("saved");
        }
        onCompositionChangeRef.current?.(composing);
      },
    });
  }
  const gate = gateRef.current;
  const flush = useCallback(() => {
    gate.flush();
  }, [gate]);

  // 5. BlockNote content changes -> debounce (suspended while composing)
  const handleEditorChange = useCallback(() => {
    gate.markDirty();
  }, [gate]);

  // 6. IME composition listeners on container (capture phase)
  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    return attachCompositionListeners(container, gate);
  }, [gate]);

  // 7. External content synchronization when same doc receives updates
  const prevDocIdRef = useRef(activeDocId);
  useEffect(() => {
    if (prevDocIdRef.current === activeDocId) {
      // Same document, check if activeInitialContent changed externally
      if (
        activeInitialContent &&
        activeInitialContent !== lastSavedJsonRef.current &&
        !gate.isDirty() &&
        !gate.isComposing()
      ) {
        const parsed = parseInitialBlocks(activeInitialContent);
        if (parsed) {
          editor.replaceBlocks(editor.document, parsed);
          lastSavedJsonRef.current = activeInitialContent;
        }
      }
    } else {
      // Switched to a new document
      prevDocIdRef.current = activeDocId;
      lastSavedJsonRef.current = activeInitialContent || "";
      gate.reset();
      setComposingStatus(false);
    }
  }, [activeDocId, activeInitialContent, editor, gate]);

  // 8. Cleanup on unmount: save anything unsaved (even mid-composition, the
  // editor is going away so no syllable can be split)
  useEffect(() => {
    return () => {
      gate.flushOnExit();
    };
  }, [gate]);

  // 9. Handle container blur (flush safe changes)
  const handleBlur = (e: React.FocusEvent<HTMLDivElement>) => {
    if (!containerRef.current?.contains(e.relatedTarget as Node)) {
      gate.flush();
    }
  };

  // 10. AI insertion support via onInsertTextRef
  useEffect(() => {
    if (onInsertTextRef) {
      onInsertTextRef.current = (text: string) => {
        try {
          const cursor = editor.getTextCursorPosition();
          editor.insertBlocks(
            [
              {
                type: "paragraph",
                content: [{ type: "text", text, styles: {} }],
              },
            ],
            cursor.block,
            "after"
          );
        } catch {
          // If no active cursor, append at the end
          const lastBlock = editor.document[editor.document.length - 1];
          editor.insertBlocks(
            [
              {
                type: "paragraph",
                content: [{ type: "text", text, styles: {} }],
              },
            ],
            lastBlock,
            "after"
          );
        }
        gate.markDirty();
        gate.flush();
      };
      return () => {
        onInsertTextRef.current = null;
      };
    }
  }, [editor, onInsertTextRef, gate]);

  // 11. Expose imperative API
  useImperativeHandle(
    ref,
    () => ({
      flush,
      isComposing: () => gate.isComposing(),
      getContent: () => ({
        content_json: JSON.stringify(editor.document),
        content_text: blocksToPlainText(editor.document),
      }),
      getEditor: () => editor,
      insertText: (text: string) => {
        try {
          const cursor = editor.getTextCursorPosition();
          editor.insertBlocks(
            [
              {
                type: "paragraph",
                content: [{ type: "text", text, styles: {} }],
              },
            ],
            cursor.block,
            "after"
          );
        } catch {
          const lastBlock = editor.document[editor.document.length - 1];
          editor.insertBlocks(
            [
              {
                type: "paragraph",
                content: [{ type: "text", text, styles: {} }],
              },
            ],
            lastBlock,
            "after"
          );
        }
        gate.markDirty();
        gate.flush();
      },
      replaceBlocks: (blocks: PartialBlock[]) => {
        editor.replaceBlocks(editor.document, blocks);
        gate.markDirty();
        gate.flush();
      },
    }),
    [editor, flush, gate]
  );

  return (
    <div
      ref={containerRef}
      onBlur={handleBlur}
      className={`relative flex flex-col flex-1 h-full w-full overflow-y-auto ${className}`}
      data-testid="editor-container"
      data-composing={composingStatus ? "true" : "false"}
    >
      <div className="flex-1 w-full max-w-4xl mx-auto py-6 px-8">
        <BlockNoteView
          editor={editor}
          theme={theme}
          editable={editable}
          onChange={handleEditorChange}
        />
      </div>

      {showStatusFooter && (
        <div className="flex items-center justify-between px-6 py-2 border-t text-xs text-muted border-border select-none">
          <div className="flex items-center gap-2">
            {composingStatus ? (
              <span className="inline-flex items-center gap-1.5 text-amber-500 font-medium">
                <span className="w-2 h-2 rounded-full bg-amber-500 animate-pulse" />
                한글 조합 중...
              </span>
            ) : gate.isDirty() ? (
              <span className="inline-flex items-center gap-1.5 text-blue-500">
                <span className="w-2 h-2 rounded-full bg-blue-500" />
                저장 대기 중
              </span>
            ) : (
              <span className="inline-flex items-center gap-1.5 text-green-500">
                <span className="w-2 h-2 rounded-full bg-green-500" />
                저장됨
              </span>
            )}
          </div>
          <div className="flex items-center gap-3">
            <span>블록 {stats.blockCount}개</span>
            <span>{stats.charCount.toLocaleString()}자</span>
          </div>
        </div>
      )}
    </div>
  );
});

export default Editor;
