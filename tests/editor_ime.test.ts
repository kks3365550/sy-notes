import { describe, it, expect } from "vitest";
import { blocksToPlainText, parseInitialBlocks } from "../src/components/Editor";

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

describe("IME Composition & 500ms Debounce State Machine Simulation", () => {
  it("should suspend auto-save during composition and flush after 500ms on compositionend", async () => {
    let isComposing = false;
    let isDirty = false;
    let debounceTimer: any = null;
    let saveCount = 0;
    let savedContent = "";

    const flush = () => {
      if (isComposing) return; // Suspended during IME!
      if (!isDirty) return;
      isDirty = false;
      saveCount++;
      savedContent = "저장된 한글 데이터";
    };

    const onCompositionStart = () => {
      isComposing = true;
      if (debounceTimer) {
        clearTimeout(debounceTimer);
        debounceTimer = null;
      }
    };

    const onContentChangeDuringIme = () => {
      isDirty = true;
      if (isComposing) {
        // Auto-save MUST be suspended during composition!
        return;
      }
      if (debounceTimer) clearTimeout(debounceTimer);
      debounceTimer = setTimeout(flush, 500);
    };

    const onCompositionEnd = () => {
      isComposing = false;
      if (isDirty) {
        if (debounceTimer) clearTimeout(debounceTimer);
        debounceTimer = setTimeout(flush, 500);
      }
    };

    // 1. User starts typing Korean syllable (e.g., 'ㄱ' -> '가' -> '강')
    onCompositionStart();
    expect(isComposing).toBe(true);

    // 2. Typing events fire during composition
    onContentChangeDuringIme();
    onContentChangeDuringIme();

    // 3. Verify no save occurred after 600ms while still composing
    await new Promise((r) => setTimeout(r, 600));
    expect(saveCount).toBe(0);

    // 4. Syllable completes: compositionend
    onCompositionEnd();
    expect(isComposing).toBe(false);

    // 5. Verify save has not happened immediately (before 500ms)
    await new Promise((r) => setTimeout(r, 200));
    expect(saveCount).toBe(0);

    // 6. Wait for 500ms debounce to complete
    await new Promise((r) => setTimeout(r, 350));
    expect(saveCount).toBe(1);
    expect(savedContent).toBe("저장된 한글 데이터");
  });
});
