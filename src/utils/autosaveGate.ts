/**
 * Autosave gate for the editor: decides *when* it is safe to save.
 *
 * Rules (Korean IME protection):
 * - While an IME composition is active, saving is suspended and any pending
 *   debounce timer is cancelled, so a half-composed syllable (e.g. "가" on the
 *   way to "강") is never written.
 * - After `compositionend`, if there are unsaved changes, save once the
 *   debounce delay (500 ms) passes without further edits.
 * - Outside composition, each edit restarts the debounce timer.
 *
 * The editor component uses this module directly, so tests exercise the real
 * logic instead of a copy of it.
 */

export const AUTOSAVE_DELAY_MS = 500;

export interface AutosaveGateOptions {
  /** Performs the actual save. Called only when it is safe to save. */
  onFlush: () => void;
  /** Debounce delay in ms (default 500). */
  delayMs?: number;
  /**
   * Extra composing signal from the editor engine (e.g. ProseMirror's
   * `view.composing`), checked in addition to composition events.
   */
  isExternallyComposing?: () => boolean;
  /** Called when composition starts (true) or ends (false). */
  onCompositionChange?: (composing: boolean) => void;
  /** Called whenever unsaved changes are waiting to be saved. */
  onPending?: () => void;
}

export interface AutosaveGate {
  /** Records an edit and (re)starts the debounce timer unless composing. */
  markDirty(): void;
  /** Handles `compositionstart`. */
  compositionStart(): void;
  /** Handles `compositionend`. */
  compositionEnd(): void;
  /** Saves now if not composing and dirty. Returns whether a save happened. */
  flush(): boolean;
  /**
   * Saves any unsaved changes immediately, even mid-composition, and cancels
   * the timer. For unmount / document switch, where no syllable can break.
   */
  flushOnExit(): void;
  /** Clears all state without saving (e.g. after loading another document). */
  reset(): void;
  isComposing(): boolean;
  isDirty(): boolean;
}

export function createAutosaveGate(options: AutosaveGateOptions): AutosaveGate {
  const delayMs = options.delayMs ?? AUTOSAVE_DELAY_MS;
  let composing = false;
  let dirty = false;
  let timer: ReturnType<typeof setTimeout> | null = null;

  const clearTimer = () => {
    if (timer !== null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  const isComposing = () => composing || Boolean(options.isExternallyComposing?.());

  const save = () => {
    clearTimer();
    dirty = false;
    options.onFlush();
  };

  const flush = (): boolean => {
    if (isComposing() || !dirty) return false;
    save();
    return true;
  };

  const schedule = () => {
    clearTimer();
    timer = setTimeout(() => {
      timer = null;
      // The engine may still report composing right after compositionend;
      // try again later instead of leaving the change unsaved.
      if (!flush() && dirty && !composing) schedule();
    }, delayMs);
  };

  return {
    markDirty() {
      dirty = true;
      options.onPending?.();
      if (isComposing()) {
        clearTimer();
        return;
      }
      schedule();
    },
    compositionStart() {
      composing = true;
      clearTimer();
      options.onCompositionChange?.(true);
    },
    compositionEnd() {
      composing = false;
      options.onCompositionChange?.(false);
      if (dirty) {
        options.onPending?.();
        schedule();
      }
    },
    flush,
    flushOnExit() {
      clearTimer();
      composing = false;
      if (dirty) save();
    },
    reset() {
      clearTimer();
      composing = false;
      dirty = false;
    },
    isComposing,
    isDirty: () => dirty,
  };
}

/**
 * Wires `compositionstart` / `compositionend` on `target` (capture phase, so
 * the editor's own handlers cannot stop them first). Returns a detach function.
 */
export function attachCompositionListeners(target: EventTarget, gate: AutosaveGate): () => void {
  const onStart = () => gate.compositionStart();
  const onEnd = () => gate.compositionEnd();
  target.addEventListener("compositionstart", onStart, true);
  target.addEventListener("compositionend", onEnd, true);
  return () => {
    target.removeEventListener("compositionstart", onStart, true);
    target.removeEventListener("compositionend", onEnd, true);
  };
}
