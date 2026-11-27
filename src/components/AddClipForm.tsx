import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { createTextClip, errorMessage, setClipText } from "../lib/ipc";
import { isMac } from "../lib/platform";
import { MarkdownToolbar } from "./MarkdownToolbar";

/** How long typing has to pause before the draft is written to the history. */
const AUTO_SAVE_DELAY_MS = 500;

type SaveStatus = "idle" | "saving" | "saved" | "seeded";

/**
 * The body of the standalone "New clip" window.
 *
 * It saves as it goes: the first pause after typing becomes a clip, and later
 * pauses rewrite that same clip, so closing the window is never a way to lose a
 * draft. "New" starts another clip in the same window, and "Copy as new" branches:
 * the clip already written stays as it is and the next edit becomes a separate
 * clip seeded with the same text.
 *
 * A clip whose text already exists is never rewritten. Auto-save notices the
 * de-duplicated insert and stops, rather than editing a row the user did not
 * open here.
 */
export function AddClipForm() {
  const areaRef = useRef<HTMLTextAreaElement | null>(null);
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<SaveStatus>("idle");

  /** The clip this editing session writes to, once one has been created. */
  const savedId = useRef<number | null>(null);
  /** What that clip holds right now, so unchanged text costs no write. */
  const lastSaved = useRef("");
  /** Text a "Copy as new" branch starts from; a no-op until it is edited. */
  const seedText = useRef<string | null>(null);
  /** The newest keystroke, read by the save loop after every write. */
  const latest = useRef("");
  /** Serialises saves so an older write can never land after a newer one. */
  const queued = useRef<Promise<void>>(Promise.resolve());

  const writeDraft = useCallback(async () => {
    while (true) {
      const value = latest.current;
      if (!value.trim()) return;
      if (seedText.current !== null) {
        // The branch has not been edited yet: there is nothing distinct to save.
        if (value === seedText.current) return;
        seedText.current = null;
      }
      if (value === lastSaved.current) return;
      setStatus("saving");
      if (savedId.current === null) {
        const saved = await createTextClip(value);
        if (!saved.created) {
          // Same text as an existing clip. Rewriting it would silently edit a
          // row this window never opened, so the draft stops here.
          lastSaved.current = value;
          setStatus("idle");
          setError(
            "That text is already in your history, so no new clip was made. Keep typing to save a different clip.",
          );
          return;
        }
        savedId.current = saved.id;
      } else {
        await setClipText(savedId.current, value);
      }
      lastSaved.current = value;
      setStatus("saved");
    }
  }, []);

  /** Waits for any write already in flight, then saves the newest text. */
  const flush = useCallback(() => {
    const run = queued.current.then(writeDraft, writeDraft);
    queued.current = run.catch(() => undefined);
    return run;
  }, [writeDraft]);

  useEffect(() => {
    latest.current = text;
    if (!text.trim() || text === lastSaved.current) return;
    if (seedText.current !== null && text === seedText.current) return;
    const timer = setTimeout(() => {
      flush().catch((e: unknown) => {
        setStatus("idle");
        setError(errorMessage(e));
      });
    }, AUTO_SAVE_DELAY_MS);
    return () => clearTimeout(timer);
  }, [text, flush]);

  const startNew = async () => {
    try {
      await flush();
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    savedId.current = null;
    lastSaved.current = "";
    seedText.current = null;
    latest.current = "";
    setText("");
    setStatus("idle");
    setError(null);
    areaRef.current?.focus();
  };

  const copyAsNew = async () => {
    if (!latest.current.trim()) {
      setError("Type something first.");
      return;
    }
    try {
      await flush();
    } catch (e) {
      setError(errorMessage(e));
      return;
    }
    // The clip just written keeps its text. This session continues from the same
    // words; the first edit is what becomes the separate clip.
    seedText.current = latest.current;
    savedId.current = null;
    lastSaved.current = "";
    setStatus("seeded");
    setError(null);
    areaRef.current?.focus();
  };

  const finish = async () => {
    try {
      await flush();
      await getCurrentWindow().close();
    } catch (e) {
      setError(errorMessage(e));
    }
  };

  const statusText =
    status === "saving"
      ? "Saving…"
      : status === "saved"
        ? "Saved"
        : status === "seeded"
          ? "Copied as a new draft — the next edit saves a separate clip"
          : "Auto-saves as you type";

  return (
    <div className="flex h-full flex-col bg-panel">
      <header className="shrink-0 border-b border-edge px-3 py-2">
        <h1 className="text-[13px] font-semibold text-ink">New clip</h1>
        <p className="mt-0.5 text-[11px] text-ink-faint">
          Markdown formatting is saved as Markdown; everything else stays plain text.
        </p>
      </header>

      <MarkdownToolbar
        areaRef={areaRef}
        onChange={setText}
        className="shrink-0 border-b border-edge bg-panel-2/50 px-3 py-1.5"
      />

      <textarea
        ref={areaRef}
        autoFocus
        value={text}
        onChange={(event) => {
          setText(event.target.value);
          setError(null);
        }}
        onBlur={() => {
          flush().catch((e: unknown) => {
            setStatus("idle");
            setError(errorMessage(e));
          });
        }}
        onKeyDown={(event) => {
          if ((event.ctrlKey || event.metaKey) && event.key === "Enter") {
            event.preventDefault();
            void finish();
          }
        }}
        spellCheck={false}
        placeholder="Type or paste the text for the new clip…"
        aria-label="Clip text"
        className="min-h-0 flex-1 resize-none border-0 bg-transparent px-3 py-2 text-[12px] leading-relaxed text-ink placeholder:text-ink-faint focus:outline-none"
      />

      {error && (
        <p role="alert" className="shrink-0 border-t border-danger/30 bg-danger/10 px-3 py-1.5 text-[11px] text-danger">
          {error}
        </p>
      )}

      <footer className="flex shrink-0 flex-wrap items-center gap-2 border-t border-edge px-3 py-2">
        <span className="mr-auto text-[10px] text-ink-faint">
          {statusText}
          <span className="ml-1.5">{isMac ? "⌘⏎ to finish" : "Ctrl+Enter to finish"}</span>
        </span>
        <button
          type="button"
          onClick={() => void startNew()}
          title="Keep this draft and start another clip"
          className="rounded border border-edge bg-panel-2 px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink"
        >
          New
        </button>
        <button
          type="button"
          onClick={() => void copyAsNew()}
          title="Keep this clip as it is and start a separate one from the same text"
          disabled={!text.trim()}
          className="rounded border border-edge bg-panel-2 px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink disabled:opacity-50"
        >
          Copy as new
        </button>
        <button
          type="button"
          onClick={() => void finish()}
          className="rounded border border-accent bg-accent px-2.5 py-1 text-[11px] font-medium text-white transition-colors hover:bg-accent/90"
        >
          Done
        </button>
      </footer>
    </div>
  );
}
