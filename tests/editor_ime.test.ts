import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { blocksToPlainText, parseInitialBlocks } from "../src/components/Editor";
import {
  AUTOSAVE_DELAY_MS,
  AutosaveGate,
  attachCompositionListeners,
  createAutosaveGate,
} from "../src/utils/autosaveGate";

describe("Editor Text Extraction & Parser Tests", () => {
  it("should extract plain text from complex block structure", () => {
    const blocks = [
      {
        id: "1",
        type: "heading",
        props: { level: 1 },
        content: [{ type: "text", text: "2026-11-22 삼영 업무일지", styles: {} }],
        children: [],
      },
      {
        id: "2",
        type: "paragraph",
        props: {},
        content: [
          { type: "text", text: "오늘의 목표: ", styles: { bold: true } },
          { type: "text", text: "스마트팩토리 센서 점검 및 ", styles: {} },
          {
            type: "link",
            href: "https://samyoung.com",
            content: [{ type: "text", text: "검교정 서버", styles: {} }],
          },
          { type: "text", text: " 배포", styles: {} },
        ],
        children: [
          {
            id: "2-1",
            type: "checkListItem",
            props: { checked: true },
            content: [{ type: "text", text: "Port 8000 헬스체크 완료", styles: {} }],
            children: [],
          },
        ],
      },
      {
        id: "3",
        type: "checkListItem",
        props: { checked: false },
        content: [{ type: "text", text: "한글 IME 받침 테스트 (완료)", styles: {} }],
        children: [],
      },
    ];

    const text = blocksToPlainText(blocks);
    expect(text).toContain("2026-11-22 삼영 업무일지");
    expect(text).toContain("스마트팩토리 센서 점검");
    expect(text).toContain("검교정 서버");
    expect(text).toContain("Port 8000 헬스체크 완료");
    expect(text).toContain("한글 IME 받침 테스트 (완료)");
  });

  it("should handle empty or malformed block structures gracefully", () => {
    expect(blocksToPlainText([])).toBe("");
    expect(blocksToPlainText(null as any)).toBe("");
    expect(blocksToPlainText(undefined as any)).toBe("");
    expect(blocksToPlainText([{ content: "simple string" }])).toBe("simple string");
  });

  it("should parse initialBlocks correctly from JSON", () => {
    const validJson = JSON.stringify([
      { id: "b1", type: "paragraph", content: [{ type: "text", text: "안녕하세요" }] },
    ]);
    const parsed = parseInitialBlocks(validJson);
    expect(Array.isArray(parsed)).toBe(true);
    expect(parsed?.length).toBe(1);
    expect(parsed?.[0].type).toBe("paragraph");

    expect(parseInitialBlocks("")).toBeUndefined();
    expect(parseInitialBlocks(null)).toBeUndefined();
    expect(parseInitialBlocks("invalid json")).toBeUndefined();
  });
});

describe("IME composition autosave gate (real Editor logic)", () => {
  // These tests drive utils/autosaveGate.ts, the exact module Editor.tsx uses,
  // with synthetic DOM CompositionEvents. Real Korean typing in WebView2 is
  // still a manual check (see tests/MANUAL_IME_CHECK.md).
  let container: HTMLDivElement;
  let saves: number;
  let gate: AutosaveGate;
  let detach: () => void;
  let engineComposing: boolean;

  const fire = (type: "compositionstart" | "compositionend") =>
    // Dispatched on a child so the capture-phase listener on the container is exercised.
    container.firstElementChild!.dispatchEvent(
      new CompositionEvent(type, { bubbles: true, data: "강" })
    );

  beforeEach(() => {
    vi.useFakeTimers();
    container = document.createElement("div");
    container.appendChild(document.createElement("p"));
    document.body.appendChild(container);
    saves = 0;
    engineComposing = false;
    gate = createAutosaveGate({
      onFlush: () => saves++,
      isExternallyComposing: () => engineComposing,
    });
    detach = attachCompositionListeners(container, gate);
  });

  afterEach(() => {
    detach();
    container.remove();
    vi.useRealTimers();
  });

  it("uses a 500ms debounce", () => {
    expect(AUTOSAVE_DELAY_MS).toBe(500);
  });

  it("debounces normal edits: saves once, 500ms after the last edit", () => {
    gate.markDirty();
    vi.advanceTimersByTime(300);
    gate.markDirty(); // restarts the timer
    vi.advanceTimersByTime(499);
    expect(saves).toBe(0);
    vi.advanceTimersByTime(1);
    expect(saves).toBe(1);
    expect(gate.isDirty()).toBe(false);
  });

  it("suspends saving during composition, then saves 500ms after compositionend", () => {
    fire("compositionstart"); // ㄱ
    expect(gate.isComposing()).toBe(true);
    gate.markDirty(); // 가
    gate.markDirty(); // 강
    vi.advanceTimersByTime(5000);
    expect(saves).toBe(0); // never saved mid-syllable

    fire("compositionend");
    expect(gate.isComposing()).toBe(false);
    vi.advanceTimersByTime(499);
    expect(saves).toBe(0);
    vi.advanceTimersByTime(1);
    expect(saves).toBe(1);
  });

  it("cancels a pending timer when composition starts", () => {
    gate.markDirty(); // timer armed
    vi.advanceTimersByTime(400);
    fire("compositionstart");
    vi.advanceTimersByTime(1000);
    expect(saves).toBe(0);
    fire("compositionend");
    vi.advanceTimersByTime(500);
    expect(saves).toBe(1);
  });

  it("blocks explicit flush (e.g. blur) while composing", () => {
    fire("compositionstart");
    gate.markDirty();
    expect(gate.flush()).toBe(false);
    expect(saves).toBe(0);
    fire("compositionend");
    expect(gate.flush()).toBe(true);
    expect(saves).toBe(1);
  });

  it("does not save after compositionend when nothing changed", () => {
    fire("compositionstart");
    fire("compositionend");
    vi.advanceTimersByTime(1000);
    expect(saves).toBe(0);
  });

  it("retries if the editor engine still reports composing after compositionend", () => {
    fire("compositionstart");
    gate.markDirty();
    engineComposing = true; // ProseMirror lags behind the DOM event
    fire("compositionend");
    vi.advanceTimersByTime(500);
    expect(saves).toBe(0);
    engineComposing = false;
    vi.advanceTimersByTime(500);
    expect(saves).toBe(1);
  });

  it("saves unsaved edits on unmount, even mid-composition", () => {
    fire("compositionstart");
    gate.markDirty();
    gate.flushOnExit();
    expect(saves).toBe(1);
    vi.advanceTimersByTime(1000);
    expect(saves).toBe(1); // no duplicate save from a stale timer
  });

  it("stops reacting to composition events after detach", () => {
    detach();
    fire("compositionstart");
    expect(gate.isComposing()).toBe(false);
    detach = () => {};
  });
});
