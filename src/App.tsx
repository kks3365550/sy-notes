import { useState, useEffect, useRef, useCallback } from "react";
import { DocumentItem, StorageInfo } from "./types";
import { Sidebar } from "./components/Sidebar";
import Editor from "./components/Editor";
import AIPanel from "./components/AIPanel";
import {
  getOrCreateDaily,
  createOrUpdateDoc,
  getStorageInfo,
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
} from "lucide-react";

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
            setCurrentDoc(daily);
            setDocTitle(daily.title);
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
  }, []);

  // When currentDoc changes, update title
  useEffect(() => {
    if (currentDoc) {
      setDocTitle(currentDoc.title);
    }
  }, [currentDoc?.id]);

  // Handle document selection from sidebar
  const handleSelectDoc = useCallback((doc: DocumentItem) => {
    setCurrentDoc(doc);
    setDocTitle(doc.title);
  }, []);

  // Handle document creation
  const handleDocCreated = useCallback((newDoc: DocumentItem) => {
    setCurrentDoc(newDoc);
    setDocTitle(newDoc.title);
    setRefreshTrigger((prev) => prev + 1);
  }, []);

  // Handle document deletion
  const handleDocDeleted = useCallback((deletedId: string) => {
    if (currentDoc?.id === deletedId) {
      getOrCreateDaily().then((daily) => {
        setCurrentDoc(daily);
        setDocTitle(daily.title);
      });
    }
    setRefreshTrigger((prev) => prev + 1);
  }, [currentDoc?.id]);

  // Handle editor save callback
  const handleEditorSave = useCallback(async (updatedDoc: DocumentItem) => {
    setSaveStatus("saving");
    try {
      await createOrUpdateDoc(updatedDoc);
      setCurrentDoc(updatedDoc);
      setSaveStatus("saved");
      setRefreshTrigger((prev) => prev + 1);
    } catch (err) {
      console.error("Failed to auto-save document:", err);
      setSaveStatus("saved");
    }
  }, []);

  // Handle title change with 400ms debounce
  const handleTitleChange = (newTitle: string) => {
    setDocTitle(newTitle);
    if (!currentDoc) return;

    if (titleDebounceTimerRef.current) {
      clearTimeout(titleDebounceTimerRef.current);
    }

    titleDebounceTimerRef.current = window.setTimeout(async () => {
      setSaveStatus("saving");
      const updated: DocumentItem = {
        ...currentDoc,
        title: newTitle.trim() || "제목 없음",
        updated_at: Date.now(),
      };
      await createOrUpdateDoc(updated);
      setCurrentDoc(updated);
      setSaveStatus("saved");
      setRefreshTrigger((prev) => prev + 1);
    }, 400);
  };

  // AI insertion callback reference
  const editorInsertRef = useRef<((text: string) => void) | null>(null);

  const handleInsertAIText = (text: string) => {
    if (editorInsertRef.current) {
      editorInsertRef.current(text);
    } else if (currentDoc) {
      // Fallback: append text
      const updated: DocumentItem = {
        ...currentDoc,
        content_text: (currentDoc.content_text || "") + "\n\n" + text,
        updated_at: Date.now(),
      };
      handleEditorSave(updated);
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

        {/* Editor Body Area */}
        <main className="flex-1 overflow-y-auto px-8 py-6 select-text">
          {currentDoc ? (
            <div className="max-w-4xl mx-auto">
              <Editor
                key={currentDoc.id}
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
