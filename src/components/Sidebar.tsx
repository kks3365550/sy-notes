import React, { useState, useEffect, useCallback, useMemo } from "react";
import { DocumentItem, StorageInfo } from "../types";
import {
  listDocs,
  listTrash,
  getOrCreateDaily,
  createOrUpdateDoc,
  deleteDoc,
  restoreDoc,
  purgeTrash,
  getStorageInfo,
} from "../api";

export interface SidebarProps {
  currentDocId?: string | null;
  onSelectDoc: (doc: DocumentItem) => void;
  onDocCreated?: (doc: DocumentItem) => void;
  onDocDeleted?: (id: string) => void;
  refreshTrigger?: number;
  className?: string;
}

// Format timestamp safely handling both seconds and milliseconds
function formatTime(timestamp: number): string {
  if (!timestamp) return "";
  const ms = timestamp < 1e11 ? timestamp * 1000 : timestamp;
  const date = new Date(ms);
  const now = new Date();

  const isToday =
    date.getFullYear() === now.getFullYear() &&
    date.getMonth() === now.getMonth() &&
    date.getDate() === now.getDate();

  const pad = (n: number) => n.toString().padStart(2, "0");
  const timeStr = `${pad(date.getHours())}:${pad(date.getMinutes())}`;

  if (isToday) {
    return `오늘 ${timeStr}`;
  }

  const yesterday = new Date(now);
  yesterday.setDate(now.getDate() - 1);
  const isYesterday =
    date.getFullYear() === yesterday.getFullYear() &&
    date.getMonth() === yesterday.getMonth() &&
    date.getDate() === yesterday.getDate();

  if (isYesterday) {
    return `어제 ${timeStr}`;
  }

  const isThisYear = date.getFullYear() === now.getFullYear();
  if (isThisYear) {
    return `${pad(date.getMonth() + 1)}/${pad(date.getDate())} ${timeStr}`;
  }

  return `${date.getFullYear()}.${pad(date.getMonth() + 1)}.${pad(date.getDate())}`;
}

export const Sidebar: React.FC<SidebarProps> = ({
  currentDocId,
  onSelectDoc,
  onDocCreated,
  onDocDeleted,
  refreshTrigger = 0,
  className = "",
}) => {
  const [docs, setDocs] = useState<DocumentItem[]>([]);
  const [trashDocs, setTrashDocs] = useState<DocumentItem[]>([]);
  const [activeTab, setActiveTab] = useState<"notes" | "trash">("notes");
  const [searchQuery, setSearchQuery] = useState("");
  const [isLoading, setIsLoading] = useState(false);
  const [actionLoadingId, setActionLoadingId] = useState<string | null>(null);
  const [storageInfo, setStorageInfo] = useState<StorageInfo | null>(null);

  useEffect(() => {
    getStorageInfo().then(setStorageInfo).catch(console.error);
  }, []);

  // Fetch active documents and trash
  const loadDocuments = useCallback(async () => {
    setIsLoading(true);
    try {
      const [fetchedDocs, fetchedTrash] = await Promise.all([
        listDocs(false),
        listTrash(),
      ]);

      // Sort by updated_at descending
      const sortedDocs = [...fetchedDocs].sort((a, b) => (b.updated_at || 0) - (a.updated_at || 0));
      const sortedTrash = [...fetchedTrash].sort((a, b) => (b.updated_at || 0) - (a.updated_at || 0));

      setDocs(sortedDocs);
      setTrashDocs(sortedTrash);
    } catch (err) {
      console.error("Failed to load documents in Sidebar:", err);
    } finally {
      setIsLoading(false);
    }
  }, []);

  // Reload when refreshTrigger changes
  useEffect(() => {
    loadDocuments();
  }, [loadDocuments, refreshTrigger]);

  // 1. [오늘 업무일지] 원클릭 생성/조회 핸들러
  const handleOpenTodayDaily = async () => {
    setActionLoadingId("daily");
    try {
      const todayDaily = await getOrCreateDaily();
      await loadDocuments();
      onSelectDoc(todayDaily);
      onDocCreated?.(todayDaily);
      setActiveTab("notes");
    } catch (err) {
      console.error("Failed to get or create daily doc:", err);
    } finally {
      setActionLoadingId(null);
    }
  };

  // 2. [+ 새 메모] 생성 핸들러
  const handleCreateNewMemo = async () => {
    setActionLoadingId("new-memo");
    try {
      const now = Date.now();
      const uuid =
        typeof crypto !== "undefined" && crypto.randomUUID
          ? crypto.randomUUID()
          : `memo-${now}-${Math.random().toString(36).substring(2, 9)}`;

      const newMemo: DocumentItem = {
        id: uuid,
        title: "제목 없는 메모",
        content: JSON.stringify([
          {
            id: `b-${now}`,
            type: "paragraph",
            props: {},
            content: [],
            children: [],
          },
        ]),
        content_text: "",
        doc_type: "memo",
        created_at: now,
        updated_at: 0, // new document: no prior version to check against
        deleted_at: null,
      };

      const saved = await createOrUpdateDoc(newMemo);
      await loadDocuments();
      onSelectDoc(saved);
      onDocCreated?.(saved);
      setActiveTab("notes");
    } catch (err) {
      console.error("Failed to create new memo:", err);
    } finally {
      setActionLoadingId(null);
    }
  };

  // 3. 소프트 삭제 핸들러
  const handleDeleteDoc = async (e: React.MouseEvent, doc: DocumentItem) => {
    e.stopPropagation();
    if (actionLoadingId) return;

    setActionLoadingId(doc.id);
    try {
      await deleteDoc(doc.id);
      await loadDocuments();
      onDocDeleted?.(doc.id);

      // If deleted doc was selected, switch selection to next available doc
      if (currentDocId === doc.id) {
        const remaining = docs.filter((d) => d.id !== doc.id);
        if (remaining.length > 0) {
          onSelectDoc(remaining[0]);
        }
      }
    } catch (err) {
      console.error("Failed to soft-delete document:", err);
    } finally {
      setActionLoadingId(null);
    }
  };

  // 4. 휴지통 복구 핸들러
  const handleRestoreDoc = async (e: React.MouseEvent, doc: DocumentItem) => {
    e.stopPropagation();
    if (actionLoadingId) return;

    setActionLoadingId(doc.id);
    try {
      await restoreDoc(doc.id);
      await loadDocuments();
      onSelectDoc(doc);
    } catch (err) {
      console.error("Failed to restore document:", err);
    } finally {
      setActionLoadingId(null);
    }
  };

  // 5. 휴지통 비우기 핸들러
  const handlePurgeTrash = async () => {
    if (trashDocs.length === 0) return;
    const confirmPurge = window.confirm(
      "휴지통의 모든 문서를 영구적으로 삭제하시겠습니까? 이 작업은 되돌릴 수 없습니다."
    );
    if (!confirmPurge) return;

    setActionLoadingId("purge");
    try {
      await purgeTrash(0);
      await loadDocuments();
    } catch (err) {
      console.error("Failed to purge trash:", err);
    } finally {
      setActionLoadingId(null);
    }
  };

  // Filtered documents by search query
  const filteredDocs = useMemo(() => {
    const list = activeTab === "notes" ? docs : trashDocs;
    if (!searchQuery.trim()) return list;

    const q = searchQuery.toLowerCase();
    return list.filter(
      (item) =>
        (item.title && item.title.toLowerCase().includes(q)) ||
        (item.content_text && item.content_text.toLowerCase().includes(q)) ||
        (item.date && item.date.includes(q))
    );
  }, [activeTab, docs, trashDocs, searchQuery]);

  // Check if today's daily log already exists
  const todayStr = useMemo(() => new Date().toISOString().split("T")[0], []);
  const todayDailyExists = useMemo(
    () => docs.some((d) => d.doc_type === "daily" && d.date === todayStr),
    [docs, todayStr]
  );

  return (
    <aside
      className={`w-64 h-full flex flex-col bg-slate-50 dark:bg-zinc-900 border-r border-slate-200 dark:border-zinc-800 select-none ${className}`}
    >
      {/* 1. Header & Quick Actions */}
      <div className="p-3 border-b border-slate-200 dark:border-zinc-800 flex flex-col gap-2">
        <div className="flex items-center justify-between">
          <div className="flex items-center gap-2">
            <span className="w-2.5 h-2.5 rounded-full bg-blue-600 dark:bg-blue-500 animate-pulse" />
            <h1 className="text-sm font-semibold tracking-tight text-slate-800 dark:text-zinc-100">
              삼영 스마트 노트
            </h1>
          </div>
          <span className="text-[11px] font-mono px-1.5 py-0.5 rounded bg-slate-200 dark:bg-zinc-800 text-slate-600 dark:text-zinc-400">
            v0.1
          </span>
        </div>

        {/* 읽기 전용 매체 감지 시 %APPDATA% 안전 저장 알림 배지 */}
        {storageInfo?.is_fallback && (
          <div
            className="flex items-start gap-1.5 p-2 rounded-md bg-amber-50 dark:bg-amber-950/40 border border-amber-200 dark:border-amber-800 text-amber-800 dark:text-amber-300 text-[11px]"
            title={`실제 저장 위치: ${storageInfo.db_path}`}
          >
            <span className="shrink-0 text-amber-500 font-bold">⚠️</span>
            <div className="flex-1 leading-tight">
              <span className="font-semibold">읽기전용 매체 실행:</span> %APPDATA%로 안전 저장 중
            </div>
          </div>
        )}

        {/* [오늘 업무일지] 원클릭 버튼 */}
        <button
          type="button"
          onClick={handleOpenTodayDaily}
          disabled={actionLoadingId === "daily"}
          className={`w-full flex items-center justify-center gap-2 py-2 px-3 rounded-lg text-xs font-medium transition-all shadow-sm ${
            todayDailyExists
              ? "bg-blue-50 hover:bg-blue-100 text-blue-700 border border-blue-200 dark:bg-blue-950/40 dark:text-blue-300 dark:border-blue-800"
              : "bg-blue-600 hover:bg-blue-700 text-white dark:bg-blue-500 dark:hover:bg-blue-600"
          }`}
          title="오늘 날짜의 업무일지를 열거나 새로 생성합니다"
        >
          {actionLoadingId === "daily" ? (
            <svg
              className="animate-spin h-3.5 w-3.5 text-current"
              xmlns="http://www.w3.org/2000/svg"
              fill="none"
              viewBox="0 0 24 24"
            >
              <circle
                className="opacity-25"
                cx="12"
                cy="12"
                r="10"
                stroke="currentColor"
                strokeWidth="4"
              />
              <path
                className="opacity-75"
                fill="currentColor"
                d="M4 12a8 8 0 018-8v8H4z"
              />
            </svg>
          ) : (
            <svg
              className="w-3.5 h-3.5"
              fill="none"
              stroke="currentColor"
              viewBox="0 0 24 24"
            >
              <rect x="3" y="4" width="18" height="18" rx="2" strokeWidth="2" />
              <path d="M16 2v4M8 2v4M3 10h18" strokeWidth="2" strokeLinecap="round" />
            </svg>
          )}
          <span>{todayDailyExists ? "오늘 업무일지 열기" : "오늘 업무일지 작성"}</span>
        </button>

        {/* [+ 새 메모] 원클릭 버튼 */}
        <button
          type="button"
          onClick={handleCreateNewMemo}
          disabled={actionLoadingId === "new-memo"}
          className="w-full flex items-center justify-center gap-1.5 py-1.5 px-3 rounded-md text-xs font-medium bg-white hover:bg-slate-100 text-slate-700 dark:bg-zinc-800 dark:hover:bg-zinc-700 dark:text-zinc-200 border border-slate-200 dark:border-zinc-700 transition-colors"
        >
          <svg
            className="w-3.5 h-3.5 text-slate-500 dark:text-zinc-400"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M12 4v16m8-8H4" />
          </svg>
          <span>새 메모</span>
        </button>
      </div>

      {/* 2. Search & Tab Filter */}
      <div className="p-2 border-b border-slate-200 dark:border-zinc-800 space-y-2">
        {/* Search input */}
        <div className="relative flex items-center">
          <svg
            className="w-3.5 h-3.5 absolute left-2.5 text-slate-400 dark:text-zinc-500 pointer-events-none"
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <circle cx="11" cy="11" r="8" strokeWidth="2" />
            <path d="M21 21l-4.35-4.35" strokeWidth="2" strokeLinecap="round" />
          </svg>
          <input
            type="text"
            placeholder="메모 필터링..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
            className="w-full pl-8 pr-6 py-1 text-xs rounded bg-white dark:bg-zinc-800 text-slate-800 dark:text-zinc-200 placeholder-slate-400 dark:placeholder-zinc-500 border border-slate-200 dark:border-zinc-700 focus:outline-none focus:ring-1 focus:ring-blue-500"
          />
          {searchQuery && (
            <button
              type="button"
              onClick={() => setSearchQuery("")}
              className="absolute right-2 text-slate-400 hover:text-slate-600 dark:hover:text-zinc-300"
            >
              <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                <path strokeLinecap="round" strokeLinejoin="round" strokeWidth="2" d="M6 18L18 6M6 6l12 12" />
              </svg>
            </button>
          )}
        </div>

        {/* View Mode Tabs: Notes vs Trash */}
        <div className="flex rounded bg-slate-200/70 dark:bg-zinc-800/70 p-0.5 text-xs">
          <button
            type="button"
            onClick={() => setActiveTab("notes")}
            className={`flex-1 py-1 px-2 rounded font-medium transition-colors flex items-center justify-center gap-1.5 ${
              activeTab === "notes"
                ? "bg-white dark:bg-zinc-700 text-slate-900 dark:text-zinc-100 shadow-xs"
                : "text-slate-500 dark:text-zinc-400 hover:text-slate-800 dark:hover:text-zinc-200"
            }`}
          >
            <span>메모</span>
            <span className="text-[10px] px-1 rounded-full bg-slate-100 dark:bg-zinc-600 text-slate-600 dark:text-zinc-300">
              {docs.length}
            </span>
          </button>
          <button
            type="button"
            onClick={() => setActiveTab("trash")}
            className={`flex-1 py-1 px-2 rounded font-medium transition-colors flex items-center justify-center gap-1.5 ${
              activeTab === "trash"
                ? "bg-white dark:bg-zinc-700 text-slate-900 dark:text-zinc-100 shadow-xs"
                : "text-slate-500 dark:text-zinc-400 hover:text-slate-800 dark:hover:text-zinc-200"
            }`}
          >
            <span>휴지통</span>
            {trashDocs.length > 0 && (
              <span className="text-[10px] px-1 rounded-full bg-red-100 dark:bg-red-900/50 text-red-600 dark:text-red-300">
                {trashDocs.length}
              </span>
            )}
          </button>
        </div>
      </div>

      {/* 3. Document List */}
      <div className="flex-1 overflow-y-auto px-1.5 py-2 space-y-1">
        {/* Trash View Header Action */}
        {activeTab === "trash" && trashDocs.length > 0 && (
          <div className="flex items-center justify-between px-2 py-1 mb-1 text-[11px] text-slate-500 dark:text-zinc-400 border-b border-slate-200 dark:border-zinc-800">
            <span>삭제된 문서 {trashDocs.length}개</span>
            <button
              type="button"
              onClick={handlePurgeTrash}
              disabled={actionLoadingId === "purge"}
              className="text-red-500 hover:text-red-700 dark:hover:text-red-400 font-medium transition-colors"
            >
              휴지통 비우기
            </button>
          </div>
        )}

        {/* Loading state */}
        {isLoading && docs.length === 0 && trashDocs.length === 0 ? (
          <div className="flex items-center justify-center py-8 text-xs text-slate-400 dark:text-zinc-500">
            <svg
              className="animate-spin h-4 w-4 mr-2 text-current"
              xmlns="http://www.w3.org/2000/svg"
              fill="none"
              viewBox="0 0 24 24"
            >
              <circle className="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="4" />
              <path className="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8v8H4z" />
            </svg>
            불러오는 중...
          </div>
        ) : filteredDocs.length === 0 ? (
          <div className="py-8 text-center text-xs text-slate-400 dark:text-zinc-500">
            {searchQuery
              ? "일치하는 메모가 없습니다."
              : activeTab === "notes"
              ? "작성된 메모가 없습니다."
              : "휴지통이 비어 있습니다."}
          </div>
        ) : (
          filteredDocs.map((doc) => {
            const isSelected = currentDocId === doc.id;
            const isDaily = doc.doc_type === "daily";
            const isBusy = actionLoadingId === doc.id;

            return (
              <div
                key={doc.id}
                onClick={() => onSelectDoc(doc)}
                className={`group relative flex flex-col gap-0.5 p-2 rounded-lg cursor-pointer transition-all ${
                  isSelected
                    ? "bg-blue-50 text-blue-900 border border-blue-200 dark:bg-blue-950/40 dark:text-blue-100 dark:border-blue-900/60"
                    : "hover:bg-slate-200/60 dark:hover:bg-zinc-800/60 text-slate-700 dark:text-zinc-200"
                }`}
              >
                {/* Title and Badges */}
                <div className="flex items-center justify-between gap-1">
                  <div className="flex items-center gap-1.5 min-w-0 flex-1">
                    {isDaily ? (
                      <span className="shrink-0 text-blue-600 dark:text-blue-400" title="업무일지">
                        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <rect x="3" y="4" width="18" height="18" rx="2" strokeWidth="2" />
                          <path d="M16 2v4M8 2v4M3 10h18" strokeWidth="2" strokeLinecap="round" />
                        </svg>
                      </span>
                    ) : (
                      <span className="shrink-0 text-slate-400 dark:text-zinc-500">
                        <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            strokeWidth="2"
                            d="M9 12h6m-6 4h6m2 5H7a2 2 0 01-2-2V5a2 2 0 012-2h5.586a1 1 0 01.707.293l5.414 5.414a1 1 0 01.293.707V19a2 2 0 01-2 2z"
                          />
                        </svg>
                      </span>
                    )}
                    <span className="text-xs font-medium truncate">
                      {doc.title || "제목 없는 메모"}
                    </span>
                  </div>

                  {/* Actions (Delete or Restore) */}
                  <div className="shrink-0 flex items-center">
                    {activeTab === "notes" ? (
                      <button
                        type="button"
                        onClick={(e) => handleDeleteDoc(e, doc)}
                        disabled={isBusy}
                        className="opacity-0 group-hover:opacity-100 p-1 rounded hover:bg-slate-300/60 dark:hover:bg-zinc-700 text-slate-400 hover:text-red-500 transition-opacity"
                        title="휴지통으로 이동"
                      >
                        <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            strokeWidth="2"
                            d="M19 7l-.867 12.142A2 2 0 0116.138 21H7.862a2 2 0 01-1.995-1.858L5 7m5 4v6m4-6v6m1-10V4a1 1 0 00-1-1h-4a1 1 0 00-1 1v3M4 7h16"
                          />
                        </svg>
                      </button>
                    ) : (
                      <button
                        type="button"
                        onClick={(e) => handleRestoreDoc(e, doc)}
                        disabled={isBusy}
                        className="p-1 rounded bg-slate-200 dark:bg-zinc-700 hover:bg-blue-100 hover:text-blue-600 dark:hover:bg-blue-900/50 dark:hover:text-blue-300 text-slate-600 dark:text-zinc-300 transition-colors text-[11px] font-medium flex items-center gap-1"
                        title="메모 목록으로 복구"
                      >
                        <svg className="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path
                            strokeLinecap="round"
                            strokeLinejoin="round"
                            strokeWidth="2"
                            d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
                          />
                        </svg>
                        <span>복구</span>
                      </button>
                    )}
                  </div>
                </div>

                {/* Snippet / Preview & Time */}
                <div className="flex items-center justify-between text-[11px] text-slate-400 dark:text-zinc-500 pl-5">
                  <span className="truncate pr-2">
                    {doc.content_text
                      ? doc.content_text.replace(/\n/g, " ").trim()
                      : isDaily
                      ? "업무일지 작성 대기"
                      : "내용 없음"}
                  </span>
                  <span className="shrink-0 font-mono text-[10px]">
                    {formatTime(doc.updated_at || doc.created_at)}
                  </span>
                </div>
              </div>
            );
          })
        )}
      </div>

      {/* 4. Footer */}
      <div className="p-2 border-t border-slate-200 dark:border-zinc-800 text-[11px] text-slate-400 dark:text-zinc-500 flex items-center justify-between">
        <span>총 {docs.length}개 메모</span>
        <button
          type="button"
          onClick={loadDocuments}
          disabled={isLoading}
          className="hover:text-slate-600 dark:hover:text-zinc-300 flex items-center gap-1"
          title="새로고침"
        >
          <svg
            className={`w-3 h-3 ${isLoading ? "animate-spin" : ""}`}
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
          >
            <path
              strokeLinecap="round"
              strokeLinejoin="round"
              strokeWidth="2"
              d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
            />
          </svg>
          <span>새로고침</span>
        </button>
      </div>
    </aside>
  );
};

export default Sidebar;
