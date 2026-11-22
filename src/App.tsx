import { useState, useEffect, useRef, useCallback } from "react";
import { DocumentItem, StorageInfo } from "./types";
import { Sidebar } from "./components/Sidebar";
import Editor from "./components/Editor";
import AIPanel from "./components/AIPanel";
import {
  getOrCreateDaily,
  getDoc,
  createOrUpdateDoc,
  getStorageInfo,
  toSaveError,
} from "./api";
import {
  Sun,
  Moon,
  Sparkles,
  CheckCircle2,
  Loader2,
  Calendar,
  FileText,
  Clock,
  AlertTriangle,
  XCircle,
} from "lucide-react";

/** Fields the UI can change on a document. */
type DocPatch = Partial<Pick<DocumentItem, "title" | "content" | "content_text">>;

interface ConflictState {
  docId: string;
  /** Version currently stored in the DB (written by another window/writer). */
  actualUpdatedAt: number | null;
}

export function App() {
  const [currentDoc, setCurrentDoc] = useState<DocumentItem | null>(null);
  const [theme, setTheme] = useState<"light" | "dark">(() => {
    const saved = localStorage.getItem("sy-notes-theme");
    if (saved === "light" || saved === "dark") return saved;
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  });
  const [isAIPanelOpen, setIsAIPanelOpen] = useState(false);
  const [saveStatus, setSaveStatus] = useState<"saved" | "saving" | "composing">("saved");
  const [refreshTrigger, setRefreshTrigger] = useState(0);
  const [docTitle, setDocTitle] = useState("");
  const [storageInfo, setStorageInfo] = useState<StorageInfo | null>(null);
  const titleDebounceTimerRef = useRef<number | null>(null);

  // --- Save pipeline -------------------------------------------------------
  // `confirmedRef` holds the latest version the backend has committed for each doc.
  // Its `updated_at` is the optimistic-lock version sent with the next save.
  // Unsaved edits are merged into `pendingRef` and written one at a time, so two
  // saves of the same document never race with the same expected version.
  const confirmedRef = useRef<Map<string, DocumentItem>>(new Map());
  const pendingRef = useRef<Map<string, DocPatch>>(new Map());
  const drainingRef = useRef(false);
  const currentDocIdRef = useRef<string | null>(null);
  currentDocIdRef.current = currentDoc?.id ?? null;
  const [conflict, setConflict] = useState<ConflictState | null>(null);
  const conflictRef = useRef<ConflictState | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const saveErrorRef = useRef<string | null>(null);
  // Bumped to remount the editor with freshly loaded content.
  const [editorRev, setEditorRev] = useState(0);
  // Ignore the outgoing editor's unmount flush while reloading from the DB.
  const suppressSavesRef = useRef(false);
  useEffect(() => {
    suppressSavesRef.current = false;
  }, [editorRev]);

  const updateConflict = (c: ConflictState | null) => {
    conflictRef.current = c;
    setConflict(c);
  };
  const updateSaveError = (msg: string | null) => {
    saveErrorRef.current = msg;
    setSaveError(msg);
  };

  /** Records a backend-confirmed document, never replacing a newer version. */
  const rememberDoc = useCallback((doc: DocumentItem): DocumentItem => {
    const known = confirmedRef.current.get(doc.id);
    if (known && known.updated_at >= doc.updated_at) return known;
    confirmedRef.current.set(doc.id, doc);
    return doc;
  }, []);

  const drainSaves = useCallback(async () => {
    if (drainingRef.current) return;
    drainingRef.current = true;
    try {
      for (;;) {
        // Documents in conflict are held until the user decides.
        const next = [...pendingRef.current.entries()].find(
          ([id]) => conflictRef.current?.docId !== id
        );
        if (!next) break;
        const [docId, patch] = next;
        pendingRef.current.delete(docId);

        const base = confirmedRef.current.get(docId);
        if (!base) continue; // document was deleted or never loaded: nothing to save

        setSaveStatus("saving");
        try {
          const saved = await createOrUpdateDoc({ ...base, ...patch });
          confirmedRef.current.set(docId, saved);
          updateSaveError(null);
          // Publish only when no newer edit is queued, so the editor is never
          // "synced back" to an older snapshot.
          if (!pendingRef.current.has(docId) && currentDocIdRef.current === docId) {
            setCurrentDoc(saved);
          }
          setRefreshTrigger((prev) => prev + 1);
        } catch (e) {
          const err = toSaveError(e);
          // Keep the edit (newer queued edits win on overlapping fields).
          pendingRef.current.set(docId, { ...patch, ...pendingRef.current.get(docId) });
          if (err.code === "conflict") {
            updateConflict({ docId, actualUpdatedAt: err.actual_updated_at });
          } else {
            console.error("Failed to save document:", err);
            updateSaveError(err.message);
            break; // retried on the next edit or via the retry button
          }
        }
      }
    } finally {
      drainingRef.current = false;
    }
    if (!conflictRef.current && !saveErrorRef.current && pendingRef.current.size === 0) {
      setSaveStatus("saved");
    }
  }, []);

  const queueSave = useCallback(
    (docId: string, patch: DocPatch) => {
      pendingRef.current.set(docId, { ...pendingRef.current.get(docId), ...patch });
      void drainSaves();
    },
    [drainSaves]
  );

  /** Conflict resolution: write my version over the one in the DB. */
  const handleOverwrite = useCallback(() => {
    const c = conflictRef.current;
    if (!c) return;
    const base = confirmedRef.current.get(c.docId);
    if (base && c.actualUpdatedAt !== null) {
      confirmedRef.current.set(c.docId, { ...base, updated_at: c.actualUpdatedAt });
    }
    updateConflict(null);
    void drainSaves();
  }, [drainSaves]);

  /** Conflict resolution: discard my unsaved edits and load the DB version. */
  const handleReloadFromDb = useCallback(async () => {
    const c = conflictRef.current;
    if (!c) return;
    pendingRef.current.delete(c.docId);
    const fresh = await getDoc(c.docId);
    updateConflict(null);
    suppressSavesRef.current = true;
    if (fresh && fresh.deleted_at === null) {
      confirmedRef.current.set(fresh.id, fresh);
      if (currentDocIdRef.current === fresh.id) {
        setCurrentDoc(fresh);
        setDocTitle(fresh.title);
      }
    } else {
      confirmedRef.current.delete(c.docId);
      const daily = rememberDoc(await getOrCreateDaily());
      setCurrentDoc(daily);
      setDocTitle(daily.title);
    }
    setEditorRev((prev) => prev + 1);
    setSaveStatus("saved");
    setRefreshTrigger((prev) => prev + 1);
  }, [rememberDoc]);

  const handleRetrySave = useCallback(() => {
    updateSaveError(null);
    void drainSaves();
  }, [drainSaves]);

  // Sync theme with document element
  useEffect(() => {
    if (theme === "dark") {
      document.documentElement.classList.add("dark");
    } else {
      document.documentElement.classList.remove("dark");
    }
    localStorage.setItem("sy-notes-theme", theme);
  }, [theme]);

  // Initial load: Open or create today's daily note & fetch storage info
  useEffect(() => {
    let mounted = true;
    async function init() {
      try {
        const [daily, storage] = await Promise.all([
          getOrCreateDaily(),
          getStorageInfo(),
        ]);
        if (mounted) {
          if (daily) {
            const known = rememberDoc(daily);
            setCurrentDoc(known);
            setDocTitle(known.title);
          }
          if (storage) {
            setStorageInfo(storage);
          }
        }
      } catch (err) {
        console.error("Failed to initialize app state:", err);
      }
    }
    init();
    return () => {
      mounted = false;
    };
  }, [rememberDoc]);

  // When currentDoc changes, update title
  useEffect(() => {
    if (currentDoc) {
      setDocTitle(currentDoc.title);
    }
  }, [currentDoc?.id]);

  // Handle document selection from sidebar
  const handleSelectDoc = useCallback((doc: DocumentItem) => {
    const known = rememberDoc(doc);
    setCurrentDoc(known);
    setDocTitle(known.title);
  }, [rememberDoc]);

  // Handle document creation
  const handleDocCreated = useCallback((newDoc: DocumentItem) => {
    const known = rememberDoc(newDoc);
    setCurrentDoc(known);
    setDocTitle(known.title);
    setRefreshTrigger((prev) => prev + 1);
  }, [rememberDoc]);

  // Handle document deletion
  const handleDocDeleted = useCallback((deletedId: string) => {
    // A queued save would otherwise resurrect the trashed document.
    pendingRef.current.delete(deletedId);
    confirmedRef.current.delete(deletedId);
    if (conflictRef.current?.docId === deletedId) updateConflict(null);
    if (currentDoc?.id === deletedId) {
      getOrCreateDaily().then((daily) => {
        const known = rememberDoc(daily);
        setCurrentDoc(known);
        setDocTitle(known.title);
      });
    }
    setRefreshTrigger((prev) => prev + 1);
  }, [currentDoc?.id, rememberDoc]);

  // Handle editor save callback
  const handleEditorSave = useCallback(
    (updatedDoc: DocumentItem) => {
      if (suppressSavesRef.current) return;
      queueSave(updatedDoc.id, {
        content: updatedDoc.content,
        content_text: updatedDoc.content_text,
      });
    },
    [queueSave]
  );

  // Handle title change with 400ms debounce
  const handleTitleChange = (newTitle: string) => {
    setDocTitle(newTitle);
    if (!currentDoc) return;

    if (titleDebounceTimerRef.current) {
      clearTimeout(titleDebounceTimerRef.current);
    }

    const docId = currentDoc.id;
    titleDebounceTimerRef.current = window.setTimeout(() => {
      queueSave(docId, { title: newTitle.trim() || "제목 없음" });
    }, 400);
  };

  // AI insertion callback reference
  const editorInsertRef = useRef<((text: string) => void) | null>(null);

  const handleInsertAIText = (text: string) => {
    if (editorInsertRef.current) {
      editorInsertRef.current(text);
    } else if (currentDoc) {
      // Fallback: append text
      queueSave(currentDoc.id, {
        content_text: (currentDoc.content_text || "") + "\n\n" + text,
      });
    }
  };

  return (
    <div className="flex h-screen w-screen overflow-hidden bg-white dark:bg-zinc-950 text-zinc-900 dark:text-zinc-100 select-none">
      {/* 1. Left Sidebar */}
      <Sidebar
        currentDocId={currentDoc?.id}
        onSelectDoc={handleSelectDoc}
        onDocCreated={handleDocCreated}
        onDocDeleted={handleDocDeleted}
        refreshTrigger={refreshTrigger}
        className="w-72 flex-shrink-0"
      />

      {/* 2. Main Content Area */}
      <div className="flex flex-col flex-1 h-full min-w-0 bg-white dark:bg-zinc-950 overflow-hidden">
        {/* Storage Fallback Warning Banner (read-only media detection) */}
        {storageInfo?.is_fallback && (
          <div className="bg-amber-500/10 dark:bg-amber-500/15 border-b border-amber-500/30 px-4 py-1.5 flex items-center justify-between text-xs text-amber-700 dark:text-amber-300 font-medium z-20">
            <div className="flex items-center gap-2">
              <AlertTriangle className="w-4 h-4 text-amber-500 shrink-0" />
              <span>⚠️ 읽기전용 폴더 실행 감지: %APPDATA%로 안전 저장 중 ({storageInfo.db_path})</span>
            </div>
            <span className="text-[11px] opacity-80">데이터 무손상 보호</span>
          </div>
        )}

        {/* Top Header Bar */}
        <header className="h-12 border-b border-zinc-200 dark:border-zinc-800 flex items-center justify-between px-6 flex-shrink-0 bg-white/70 dark:bg-zinc-950/70 backdrop-blur-sm z-10">
          {/* Document metadata & editable title */}
          <div className="flex items-center gap-3 min-w-0 flex-1">
            <div className="p-1.5 rounded-md bg-zinc-100 dark:bg-zinc-800 text-zinc-500">
              {currentDoc?.doc_type === "daily" ? (
                <Calendar className="w-4 h-4 text-blue-500" />
              ) : (
                <FileText className="w-4 h-4 text-zinc-400" />
              )}
            </div>

            <input
              type="text"
              value={docTitle}
              onChange={(e) => handleTitleChange(e.target.value)}
              placeholder="문서 제목을 입력하세요..."
              className="text-sm font-semibold bg-transparent border-none outline-none focus:ring-1 focus:ring-blue-500/40 rounded px-1.5 py-0.5 truncate max-w-md text-zinc-800 dark:text-zinc-200"
            />

            {/* Save Status Badge */}
            <div className="flex items-center gap-1.5 text-xs select-none pl-2">
              {conflict ? (
                <span className="flex items-center gap-1 text-red-600 dark:text-red-400 font-medium">
                  <AlertTriangle className="w-3.5 h-3.5" />
                  저장 충돌
                </span>
              ) : saveError ? (
                <span className="flex items-center gap-1 text-red-600 dark:text-red-400 font-medium">
                  <XCircle className="w-3.5 h-3.5" />
                  저장 실패
                </span>
              ) : (
              <>
              {saveStatus === "saving" && (
                <span className="flex items-center gap-1 text-amber-500 dark:text-amber-400 font-medium animate-pulse">
                  <Loader2 className="w-3.5 h-3.5 animate-spin" />
                  저장 중...
                </span>
              )}
              {saveStatus === "composing" && (
                <span className="flex items-center gap-1 text-purple-500 dark:text-purple-400 font-medium">
                  <Clock className="w-3.5 h-3.5" />
                  한글 조합 중 (저장 보류)
                </span>
              )}
              {saveStatus === "saved" && (
                <span className="flex items-center gap-1 text-emerald-600 dark:text-emerald-400">
                  <CheckCircle2 className="w-3.5 h-3.5" />
                  저장됨
                </span>
              )}
              </>
              )}
            </div>
          </div>

          {/* Header Action Tools */}
          <div className="flex items-center gap-2">
            {/* AI Assistant Button */}
            <button
              type="button"
              onClick={() => setIsAIPanelOpen((prev) => !prev)}
              className={`flex items-center gap-1.5 px-2.5 py-1.5 rounded-md text-xs font-medium border transition-all ${
                isAIPanelOpen
                  ? "bg-blue-50 dark:bg-blue-950/40 text-blue-600 dark:text-blue-400 border-blue-300 dark:border-blue-800"
                  : "bg-zinc-100 dark:bg-zinc-800/80 text-zinc-700 dark:text-zinc-300 border-zinc-200 dark:border-zinc-700 hover:bg-zinc-200/80 dark:hover:bg-zinc-700"
              }`}
              title="AI 어시스턴트 토글 (Ctrl+J)"
            >
              <Sparkles className="w-3.5 h-3.5 text-blue-500" />
              <span>AI</span>
              <kbd className="text-[10px] font-mono px-1 py-0.2 rounded bg-zinc-200 dark:bg-zinc-700 text-zinc-500">
                Ctrl+J
              </kbd>
            </button>

            {/* Theme Toggle Button */}
            <button
              type="button"
              onClick={() => setTheme((prev) => (prev === "dark" ? "light" : "dark"))}
              className="p-1.5 rounded-md border border-zinc-200 dark:border-zinc-800 text-zinc-500 hover:text-zinc-800 dark:hover:text-zinc-200 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
              title={theme === "dark" ? "라이트 모드로 전환" : "다크 모드로 전환"}
            >
              {theme === "dark" ? <Sun className="w-4 h-4" /> : <Moon className="w-4 h-4" />}
            </button>
          </div>
        </header>

        {/* Save conflict: another window/writer changed this document */}
        {conflict && (
          <div
            role="alert"
            className="bg-red-500/10 border-b border-red-500/30 px-4 py-2 flex flex-wrap items-center justify-between gap-2 text-xs text-red-700 dark:text-red-300 z-20"
          >
            <div className="flex items-center gap-2">
              <AlertTriangle className="w-4 h-4 text-red-500 shrink-0" />
              <span>
                다른 창에서 수정됨: 이 문서가 다른 곳에서 먼저 저장되어 내 변경 사항을 저장하지 못했습니다.
              </span>
            </div>
            <div className="flex items-center gap-2">
              <button
                type="button"
                onClick={handleOverwrite}
                className="px-2.5 py-1 rounded-md bg-red-600 text-white font-medium hover:bg-red-700"
              >
                내 내용으로 덮어쓰기
              </button>
              <button
                type="button"
                onClick={() => void handleReloadFromDb()}
                className="px-2.5 py-1 rounded-md border border-red-500/40 font-medium hover:bg-red-500/10"
              >
                저장된 내용 불러오기
              </button>
            </div>
          </div>
        )}

        {/* Save failure (non-conflict): edits are kept and retried */}
        {!conflict && saveError && (
          <div
            role="alert"
            className="bg-red-500/10 border-b border-red-500/30 px-4 py-2 flex flex-wrap items-center justify-between gap-2 text-xs text-red-700 dark:text-red-300 z-20"
          >
            <div className="flex items-center gap-2 min-w-0">
              <XCircle className="w-4 h-4 text-red-500 shrink-0" />
              <span className="truncate">저장 실패: {saveError} (변경 내용은 보관 중입니다)</span>
            </div>
            <button
              type="button"
              onClick={handleRetrySave}
              className="px-2.5 py-1 rounded-md border border-red-500/40 font-medium hover:bg-red-500/10"
            >
              다시 시도
            </button>
          </div>
        )}

        {/* Editor Body Area */}
        <main className="flex-1 overflow-y-auto px-8 py-6 select-text">
          {currentDoc ? (
            <div className="max-w-4xl mx-auto">
              <Editor
                key={`${currentDoc.id}:${editorRev}`}
                document={currentDoc}
                onSave={handleEditorSave}
                theme={theme}
                onStatusChange={setSaveStatus}
                onInsertTextRef={editorInsertRef}
              />
            </div>
          ) : (
            <div className="h-full flex items-center justify-center text-zinc-400 text-sm">
              <p>문서를 선택하거나 새로운 메모를 생성하세요.</p>
            </div>
          )}
        </main>
      </div>

      {/* 3. Right AI Panel */}
      <AIPanel
        isOpen={isAIPanelOpen}
        onClose={() => setIsAIPanelOpen(false)}
        onToggle={() => setIsAIPanelOpen((prev) => !prev)}
        contextText={currentDoc?.content_text || ""}
        onInsertText={handleInsertAIText}
      />
    </div>
  );
}

export default App;
