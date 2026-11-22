import React, { useState, useEffect, useRef, useCallback } from "react";
import { Channel, invoke, isTauri } from "@tauri-apps/api/core";

export interface AIPanelProps {
  /** Controls panel visibility from outside (optional) */
  isOpen?: boolean;
  /** Callback when panel requests to close */
  onClose?: () => void;
  /** Callback when panel requests to toggle */
  onToggle?: () => void;
  /** Current editor/document context string for contextual queries */
  contextText?: string;
  /** Callback to insert generated AI response into the document/editor */
  onInsertText?: (text: string) => void;
}

export function AIPanel({
  isOpen: propIsOpen,
  onClose,
  onToggle,
  contextText = "",
  onInsertText,
}: AIPanelProps) {
  // Controlled vs uncontrolled open state
  const [internalIsOpen, setInternalIsOpen] = useState(false);
  const isControlled = propIsOpen !== undefined;
  const isPanelOpen = isControlled ? propIsOpen : internalIsOpen;

  const [prompt, setPrompt] = useState("");
  const [response, setResponse] = useState("");
  const [isStreaming, setIsStreaming] = useState(false);
  const [isCancelled, setIsCancelled] = useState(false);
  const [errorMsg, setErrorMsg] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const responseContainerRef = useRef<HTMLDivElement>(null);
  const mockIntervalRef = useRef<number | null>(null);

  // Close or toggle helper
  const handleToggle = useCallback(() => {
    if (onToggle) {
      onToggle();
    } else if (isControlled) {
      if (isPanelOpen) onClose?.();
    } else {
      setInternalIsOpen((prev) => !prev);
    }
  }, [onToggle, isControlled, isPanelOpen, onClose]);

  const handleClose = useCallback(() => {
    if (onClose) {
      onClose();
    } else {
      setInternalIsOpen(false);
    }
  }, [onClose]);

  // Global Ctrl+J / Cmd+J toggle and Escape close handler
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && (e.key === "j" || e.key === "J")) {
        e.preventDefault();
        handleToggle();
      } else if (e.key === "Escape" && isPanelOpen) {
        e.preventDefault();
        handleClose();
      }
    };

    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [handleToggle, handleClose, isPanelOpen]);

  // Focus prompt input when panel opens
  useEffect(() => {
    if (isPanelOpen) {
      setTimeout(() => {
        textareaRef.current?.focus();
      }, 100);
    }
  }, [isPanelOpen]);

  // Auto-scroll response view to bottom during streaming
  useEffect(() => {
    if (responseContainerRef.current) {
      responseContainerRef.current.scrollTop =
        responseContainerRef.current.scrollHeight;
    }
  }, [response, isStreaming]);

  // Clean up mock stream interval on unmount
  useEffect(() => {
    return () => {
      if (mockIntervalRef.current) {
        clearInterval(mockIntervalRef.current);
      }
    };
  }, []);

  // Cancel active stream
  const handleCancel = async () => {
    if (!isStreaming) return;

    if (mockIntervalRef.current) {
      clearInterval(mockIntervalRef.current);
      mockIntervalRef.current = null;
    }

    try {
      if (isTauri()) {
        await invoke("cancel_ai_stream");
      }
    } catch (err) {
      console.warn("cancel_ai_stream invoke error:", err);
    } finally {
      setIsStreaming(false);
      setIsCancelled(true);
      setResponse((prev) => prev + "\n\n⚠️ [스트리밍이 취소되었습니다]");
    }
  };

  // Execute AI generation with streaming
  const handleSend = async (customPrompt?: string, customContext?: string) => {
    const finalPrompt = (customPrompt ?? prompt).trim();
    if (!finalPrompt || isStreaming) return;

    const finalContext = customContext !== undefined ? customContext : contextText;

    // Reset state for new run
    setResponse("");
    setErrorMsg(null);
    setIsCancelled(false);
    setIsStreaming(true);

    // 1. Browser/Dev Fallback (Mock streaming if not in real Tauri environment)
    if (!isTauri()) {
      console.info("[AIPanel] Non-Tauri environment: running simulated stream.");
      const mockReply =
        `[개발 모드 시뮬레이션 응답]\n\n` +
        `요청: "${finalPrompt}"\n\n` +
        (finalContext ? `참조 문맥 (${finalContext.length}자): "${finalContext.slice(0, 80)}..."\n\n` : "") +
        `1. 핵심 사항을 성공적으로 파악했습니다.\n` +
        `2. 로컬 백엔드(Ollama 127.0.0.1:11434)와 Tauri v2 Channel을 통해 실시간 토큰이 스트리밍됩니다.\n` +
        `3. 취소 버튼 클릭 시 즉각적인 CancellationToken drop 처리가 보장됩니다.`;

      let index = 0;
      mockIntervalRef.current = window.setInterval(() => {
        if (index < mockReply.length) {
          const sliceSize = Math.floor(Math.random() * 3) + 1;
          const chunk = mockReply.slice(index, index + sliceSize);
          setResponse((prev) => prev + chunk);
          index += sliceSize;
        } else {
          if (mockIntervalRef.current) {
            clearInterval(mockIntervalRef.current);
            mockIntervalRef.current = null;
          }
          setIsStreaming(false);
        }
      }, 35);
      return;
    }

    // 2. Production Tauri v2 Channel Subscription
    try {
      const channel = new Channel<string>();

      channel.onmessage = (token: string) => {
        setResponse((prev) => prev + token);
      };

      await invoke("generate_ai_stream", {
        prompt: finalPrompt,
        context: finalContext ? finalContext : null,
        onEvent: channel,
      });
    } catch (err: any) {
      console.error("generate_ai_stream error:", err);
      const errorString =
        typeof err === "string"
          ? err
          : err?.message || "AI 서버와 통신 중 오류가 발생했습니다. (Ollama 기동 여부를 확인하세요)";
      setErrorMsg(errorString);
    } finally {
      setIsStreaming(false);
    }
  };

  // Quick Action: 3줄 요약
  const handleQuickSummary = () => {
    const summaryPrompt = "현재 문서의 핵심 내용을 3줄로 명확하게 요약해 주세요.";
    setPrompt(summaryPrompt);
    handleSend(summaryPrompt, contextText);
  };

  // Quick Action: 문맥 질의
  const handleQuickContextQuery = () => {
    const queryPrefix = "현재 문서 내용을 기반으로 다음 질문에 답변해 주세요: ";
    setPrompt(queryPrefix);
    if (textareaRef.current) {
      textareaRef.current.focus();
      textareaRef.current.value = queryPrefix;
    }
  };

  // Quick Action: 맞춤법 및 문장 교정
  const handleQuickProofread = () => {
    const proofreadPrompt = "다음 문맥의 맞춤법, 띄어쓰기를 교정하고 더 자연스럽고 명확한 업무 문장으로 다듬어 주세요.";
    setPrompt(proofreadPrompt);
    handleSend(proofreadPrompt, contextText);
  };

  // Quick Action: 할 일(Action Items) 추출
  const handleQuickExtractTodos = () => {
    const todoPrompt = "문서 내용에서 실행해야 할 구체적인 할 일(Action Items)과 담당/기한을 체크리스트 형태로 추출해 주세요.";
    setPrompt(todoPrompt);
    handleSend(todoPrompt, contextText);
  };

  // Copy response to clipboard
  const handleCopy = async () => {
    if (!response) return;
    try {
      await navigator.clipboard.writeText(response);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      console.error("Clipboard copy failed:", err);
    }
  };

  // Insert response into document editor
  const handleInsert = () => {
    if (!response || !onInsertText) return;
    onInsertText(response);
  };

  // Keydown in prompt textarea: Enter to send (Shift+Enter for newline)
  const handleTextareaKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      // Avoid triggering send during Korean IME composition
      if ((e.nativeEvent as any).isComposing) {
        return;
      }
      e.preventDefault();
      handleSend();
    }
  };

  if (!isPanelOpen) {
    return null;
  }

  return (
    <div
      role="dialog"
      aria-label="AI 어시스턴트 패널"
      className="fixed inset-y-0 right-0 z-50 w-full max-w-[420px] bg-white dark:bg-zinc-900 border-l border-zinc-200 dark:border-zinc-800 shadow-2xl flex flex-col font-sans transition-all duration-200 ease-in-out select-text"
      style={{
        backgroundColor: "var(--card, #18181b)",
        color: "var(--foreground, #f4f4f5)",
        borderColor: "var(--border, #27272a)",
      }}
    >
      {/* 1. Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-200 dark:border-zinc-800 select-none">
        <div className="flex items-center gap-2">
          {/* AI Sparkles Icon */}
          <div className="p-1.5 rounded-lg bg-blue-500/10 text-blue-500 dark:text-blue-400">
            <svg
              className="w-5 h-5"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
              strokeWidth={2}
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                d="M9.813 15.904L9 18.75l-.813-2.846a4.5 4.5 0 00-3.09-3.09L2.25 12l2.846-.813a4.5 4.5 0 003.09-3.09L9 5.25l.813 2.846a4.5 4.5 0 003.09 3.09L15.75 12l-2.846.813a4.5 4.5 0 00-3.09 3.09zM18.259 8.715L18 9.75l-.259-1.035a3.375 3.375 0 00-2.455-2.456L14.25 6l1.036-.259a3.375 3.375 0 002.455-2.456L18 2.25l.259 1.035a3.375 3.375 0 002.456 2.456L21.75 6l-1.035.259a3.375 3.375 0 00-2.456 2.456zM16.894 20.567L16.5 21.75l-.394-1.183a2.25 2.25 0 00-1.423-1.423L13.5 18.75l1.183-.394a2.25 2.25 0 001.423-1.423l.394-1.183.394 1.183a2.25 2.25 0 001.423 1.423l1.183.394-1.183.394a2.25 2.25 0 00-1.423 1.423z"
              />
            </svg>
          </div>
          <div>
            <h2 className="text-sm font-semibold tracking-tight flex items-center gap-1.5">
              SY AI 어시스턴트
              <span className="text-[10px] font-mono font-normal px-1.5 py-0.5 rounded bg-zinc-100 dark:bg-zinc-800 text-zinc-500 border border-zinc-200 dark:border-zinc-700">
                Ctrl+J
              </span>
              {isCancelled && (
                <span className="text-[10px] font-medium px-1.5 py-0.5 rounded bg-amber-500/10 text-amber-500 border border-amber-500/20">
                  중단됨
                </span>
              )}
            </h2>
          </div>
        </div>

        <div className="flex items-center gap-1">
          {/* Close button */}
          <button
            type="button"
            onClick={handleClose}
            className="p-1.5 rounded-md hover:bg-zinc-100 dark:hover:bg-zinc-800 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200 transition-colors"
            title="닫기 (Esc)"
          >
            <svg className="w-4 h-4" fill="none" viewBox="0 0 24 24" stroke="currentColor">
              <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
            </svg>
          </button>
        </div>
      </div>

      {/* 2. Context Indicator (if context exists) */}
      {contextText && (
        <div className="px-4 py-2 bg-blue-50/50 dark:bg-blue-950/20 border-b border-blue-100 dark:border-blue-900/30 text-xs text-blue-700 dark:text-blue-300 flex items-center justify-between select-none">
          <span className="truncate max-w-[280px]">
            📄 참조 문맥: {contextText.slice(0, 45)}... ({contextText.length}자)
          </span>
          <span className="text-[10px] font-medium opacity-80">연동됨</span>
        </div>
      )}

      {/* 3. Quick Action Buttons */}
      <div className="p-3 border-b border-zinc-200 dark:border-zinc-800 bg-zinc-50/50 dark:bg-zinc-900/50 select-none">
        <div className="text-[11px] font-medium text-zinc-500 dark:text-zinc-400 mb-2 flex items-center gap-1">
          <span>⚡ 빠른 실행</span>
        </div>
        <div className="grid grid-cols-2 gap-2">
          {/* [3줄 요약] 버튼 */}
          <button
            type="button"
            onClick={handleQuickSummary}
            disabled={isStreaming}
            className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 hover:bg-blue-50 dark:hover:bg-zinc-700/80 hover:border-blue-300 dark:hover:border-blue-500 transition-all text-left shadow-sm disabled:opacity-50"
          >
            <span className="text-blue-500 font-bold">📑</span>
            <span className="truncate">3줄 요약</span>
          </button>

          {/* [문맥 질의] 버튼 */}
          <button
            type="button"
            onClick={handleQuickContextQuery}
            disabled={isStreaming}
            className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 hover:bg-blue-50 dark:hover:bg-zinc-700/80 hover:border-blue-300 dark:hover:border-blue-500 transition-all text-left shadow-sm disabled:opacity-50"
          >
            <span className="text-amber-500 font-bold">💬</span>
            <span className="truncate">문맥 질의</span>
          </button>

          {/* [교정/다듬기] 버튼 */}
          <button
            type="button"
            onClick={handleQuickProofread}
            disabled={isStreaming}
            className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 hover:bg-blue-50 dark:hover:bg-zinc-700/80 hover:border-blue-300 dark:hover:border-blue-500 transition-all text-left shadow-sm disabled:opacity-50"
          >
            <span className="text-emerald-500 font-bold">✏️</span>
            <span className="truncate">교정 및 다듬기</span>
          </button>

          {/* [할 일 추출] 버튼 */}
          <button
            type="button"
            onClick={handleQuickExtractTodos}
            disabled={isStreaming}
            className="flex items-center gap-1.5 px-2.5 py-1.5 text-xs font-medium rounded-md border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 hover:bg-blue-50 dark:hover:bg-zinc-700/80 hover:border-blue-300 dark:hover:border-blue-500 transition-all text-left shadow-sm disabled:opacity-50"
          >
            <span className="text-purple-500 font-bold">📋</span>
            <span className="truncate">할 일 추출</span>
          </button>
        </div>
      </div>

      {/* 4. Streaming Response Area */}
      <div
        ref={responseContainerRef}
        className="flex-1 overflow-y-auto p-4 space-y-3 text-sm leading-relaxed"
      >
        {!response && !errorMsg && !isStreaming && (
          <div className="h-full flex flex-col items-center justify-center text-center text-zinc-400 dark:text-zinc-500 p-6">
            <svg
              className="w-10 h-10 mb-3 opacity-40"
              fill="none"
              viewBox="0 0 24 24"
              stroke="currentColor"
            >
              <path
                strokeLinecap="round"
                strokeLinejoin="round"
                strokeWidth={1.5}
                d="M8 12h.01M12 12h.01M16 12h.01M21 12c0 4.418-4.03 8-9 8a9.863 9.863 0 01-4.255-.949L3 20l1.395-3.72C3.512 15.042 3 13.574 3 12c0-4.418 4.03-8 9-8s9 3.582 9 8z"
              />
            </svg>
            <p className="text-xs font-medium">질문이나 지시를 입력하세요.</p>
            <p className="text-[11px] mt-1 text-zinc-400">
              상단의 퀵 액션 또는 입력창에서 직접 질의할 수 있습니다.
            </p>
          </div>
        )}

        {/* Error notification */}
        {errorMsg && (
          <div className="p-3 rounded-lg bg-red-50 dark:bg-red-950/30 border border-red-200 dark:border-red-900/50 text-red-600 dark:text-red-400 text-xs">
            <div className="font-semibold flex items-center gap-1.5 mb-1">
              <span>⚠️ 통신 오류</span>
            </div>
            <p className="whitespace-pre-wrap">{errorMsg}</p>
          </div>
        )}

        {/* Response display with live streaming cursor */}
        {response && (
          <div className="rounded-lg p-3 bg-zinc-50 dark:bg-zinc-800/40 border border-zinc-200 dark:border-zinc-800 whitespace-pre-wrap font-sans text-xs sm:text-sm">
            {response}
            {isStreaming && (
              <span className="inline-block w-1.5 h-4 ml-0.5 align-middle bg-blue-500 animate-pulse" />
            )}
          </div>
        )}

        {/* Streaming active indicator */}
        {isStreaming && (
          <div className="flex items-center gap-2 text-xs text-blue-500 font-medium select-none">
            <span className="relative flex h-2 w-2">
              <span className="animate-ping absolute inline-flex h-full w-full rounded-full bg-blue-400 opacity-75"></span>
              <span className="relative inline-flex rounded-full h-2 w-2 bg-blue-500"></span>
            </span>
            <span>답변을 실시간으로 작성하는 중...</span>
          </div>
        )}
      </div>

      {/* 5. Action Footer for Response (Copy / Insert into doc) */}
      {response && !isStreaming && (
        <div className="px-4 py-2 border-t border-zinc-200 dark:border-zinc-800 flex items-center justify-between text-xs bg-zinc-50/50 dark:bg-zinc-900/50 select-none">
          <button
            type="button"
            onClick={() => {
              setResponse("");
              setErrorMsg(null);
            }}
            className="text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-300"
          >
            대화 지우기
          </button>

          <div className="flex items-center gap-2">
            <button
              type="button"
              onClick={handleCopy}
              className="px-2.5 py-1 rounded border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-zinc-800 hover:bg-zinc-100 dark:hover:bg-zinc-700 transition-colors flex items-center gap-1 font-medium"
            >
              {copied ? "✓ 복사됨" : "복사"}
            </button>
            {onInsertText && (
              <button
                type="button"
                onClick={handleInsert}
                className="px-2.5 py-1 rounded bg-blue-600 hover:bg-blue-700 text-white font-medium transition-colors flex items-center gap-1 shadow-sm"
              >
                본문에 삽입
              </button>
            )}
          </div>
        </div>
      )}

      {/* 6. Prompt Input & Controls */}
      <div className="p-3 border-t border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900">
        <div className="relative rounded-lg border border-zinc-300 dark:border-zinc-700 focus-within:border-blue-500 dark:focus-within:border-blue-400 transition-all bg-zinc-50/40 dark:bg-zinc-800/40">
          <textarea
            ref={textareaRef}
            rows={3}
            value={prompt}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={handleTextareaKeyDown}
            placeholder="AI에게 질문이나 지시를 입력하세요 (Enter: 전송, Shift+Enter: 줄바꿈)..."
            disabled={isStreaming}
            className="w-full resize-none bg-transparent px-3 py-2 text-xs sm:text-sm placeholder-zinc-400 focus:outline-none disabled:opacity-50"
          />

          <div className="flex items-center justify-between px-3 py-1.5 border-t border-zinc-200/50 dark:border-zinc-700/50 text-xs">
            <span className="text-[11px] text-zinc-400">
              {prompt.length > 0 ? `${prompt.length}자` : "Enter로 즉시 전송"}
            </span>

            <div className="flex items-center gap-1.5">
              {/* [취소] 버튼 (스트리밍 중일 때 표시) */}
              {isStreaming ? (
                <button
                  type="button"
                  onClick={handleCancel}
                  className="flex items-center gap-1 px-3 py-1 rounded-md text-xs font-semibold bg-red-600 hover:bg-red-700 text-white transition-colors shadow-sm"
                  title="스트리밍 즉시 중단"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <rect x="6" y="6" width="12" height="12" rx="2" fill="currentColor" />
                  </svg>
                  취소
                </button>
              ) : (
                /* [전송] 버튼 */
                <button
                  type="button"
                  onClick={() => handleSend()}
                  disabled={!prompt.trim() || isStreaming}
                  className="flex items-center gap-1 px-3 py-1 rounded-md text-xs font-semibold bg-blue-600 hover:bg-blue-700 text-white disabled:opacity-40 disabled:cursor-not-allowed transition-colors shadow-sm"
                >
                  <svg className="w-3.5 h-3.5" fill="none" viewBox="0 0 24 24" stroke="currentColor">
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M5 10l7-7m0 0l7 7m-7-7v18"
                    />
                  </svg>
                  전송
                </button>
              )}
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

export default AIPanel;
