/**
 * The `::` popup.
 *
 * It opens on the clip last used and grows downward through the history: pressing
 * Down reveals the ten most recent copies, and typing two characters turns it into
 * a search over the whole history plus any emoji whose name matches. Enter pastes
 * into the window the popup was opened from, which the backend remembered before
 * this window took the focus.
 *
 * The window has no chrome, so the list is the whole interface and every key
 * matters: Down and Up move, Enter commits, Escape leaves the document untouched.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useSettings } from "../hooks/useSettings";
import { searchEmoji } from "../lib/emoji";
import {
  errorMessage,
  quickClose,
  quickCommitClip,
  quickCommitText,
  quickItems,
} from "../lib/ipc";
import { applyTheme } from "../lib/theme";
import type { QuickItem } from "../lib/types";
import { IconShield, IconText } from "./icons";

/** Row height in CSS pixels; also what the window height is computed from. */
const ROW_HEIGHT = 30;
const HEADER_HEIGHT = 40;
const FOOTER_HEIGHT = 22;
const WINDOW_WIDTH = 380;
/** Rows shown at most, matching the backend's default limit. */
const MAX_ROWS = 10;
/** Emoji compete with clips for the same rows, so they get the smaller share. */
const MAX_EMOJI = 4;
/** A search that fired on every keystroke would run a scan per character. */
const SEARCH_DELAY = 120;

function windowHeight(rows: number): number {
  return HEADER_HEIGHT + Math.max(rows, 1) * ROW_HEIGHT + FOOTER_HEIGHT;
}

/**
 * The Tauri window handle, or null when this page is being looked at in a plain
 * browser — the dev server serves the same bundle, and asking Tauri for a window
 * there throws before any `.catch` can see it.
 */
function currentWindow(): ReturnType<typeof getCurrentWindow> | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

function Row({
  item,
  active,
  onCommit,
  onHover,
}: {
  item: QuickItem;
  active: boolean;
  onCommit: () => void;
  onHover: () => void;
}) {
  return (
    <button
      type="button"
      onMouseEnter={onHover}
      onClick={onCommit}
      className={`flex w-full items-center gap-2 px-2 text-left text-[11px] transition-colors ${
        active ? "bg-accent/20 text-ink" : "text-ink-dim hover:bg-panel-2"
      }`}
      style={{ height: ROW_HEIGHT }}
    >
      <span className="flex h-5 w-7 shrink-0 items-center justify-center rounded border border-edge-soft bg-panel-3 text-ink-faint">
        {item.kind === "emoji" ? (
          <span className="text-[13px] leading-none">{item.label}</span>
        ) : item.secret ? (
          <IconShield className="size-2.5" />
        ) : (
          <IconText className="size-2.5" />
        )}
      </span>
      {item.kind === "emoji" ? (
        <span className="min-w-0 flex-1 truncate">
          <span className="mr-1 text-[13px]">{item.label}</span>
          <span className="text-ink-faint">{item.detail}</span>
        </span>
      ) : (
        <span className="min-w-0 flex-1 truncate" title={item.label}>
          {item.label || "(empty)"}
        </span>
      )}
      {item.detail && item.kind !== "emoji" && (
        <span className="shrink-0 text-[10px] text-ink-faint">{item.detail}</span>
      )}
    </button>
  );
}

export function QuickPicker() {
  const { settings } = useSettings();
  const [query, setQuery] = useState("");
  const [clips, setClips] = useState<QuickItem[]>([]);
  const [expanded, setExpanded] = useState(false);
  const [selected, setSelected] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    if (settings?.theme) applyTheme(settings.theme);
  }, [settings?.theme]);

  // The panel is built by the backend at a fixed width; the height is this
  // component's business, because only it knows how many rows there are.
  const resize = useCallback((rows: number) => {
    currentWindow()
      ?.setSize(new LogicalSize(WINDOW_WIDTH, windowHeight(rows)))
      .catch(() => undefined);
  }, []);

  const dismiss = useCallback(() => {
    // The window is closed from the backend; if that call cannot be made, closing
    // it from here is the same result.
    quickClose().catch(() => {
      currentWindow()
        ?.close()
        .catch(() => undefined);
    });
  }, []);

  // Nothing typed is a listing, not a search, so it is fetched at once; anything
  // typed waits for the delay.
  useEffect(() => {
    let cancelled = false;
    const term = query.trim();
    const wait = term.length >= 2 ? SEARCH_DELAY : 0;
    const timer = window.setTimeout(() => {
      quickItems(term, MAX_ROWS)
        .then((next) => {
          if (!cancelled) setClips(next);
        })
        .catch((e) => {
          if (!cancelled) setError(errorMessage(e));
        });
    }, wait);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [query]);

  const searching = query.trim().length >= 2;

  const emoji = useMemo<QuickItem[]>(() => {
    if (!searching || !settings?.quickPasteEmoji) return [];
    return searchEmoji(query)
      .slice(0, MAX_EMOJI)
      .map((entry) => ({
        id: 0,
        kind: "emoji" as const,
        label: entry.char,
        detail: entry.name,
        secret: false,
      }));
  }, [query, searching, settings?.quickPasteEmoji]);

  const rows = useMemo(() => {
    if (searching) return [...clips, ...emoji].slice(0, MAX_ROWS);
    // Unopened, the popup is a single row: the clip copied a moment ago.
    return expanded ? clips.slice(0, MAX_ROWS) : clips.slice(0, 1);
  }, [clips, emoji, expanded, searching]);

  useEffect(() => {
    resize(rows.length);
  }, [rows.length, resize]);

  useEffect(() => {
    setSelected((current) => Math.min(current, Math.max(rows.length - 1, 0)));
  }, [rows.length]);

  // A new search is a new list: keeping the old highlight would make Enter commit
  // whatever happens to sit at that index now.
  useEffect(() => {
    setSelected(0);
  }, [query]);

  const commit = useCallback(
    (index: number) => {
      const item = rows[index];
      if (!item || busy) return;
      setBusy(true);
      setError(null);
      // A successful commit closes this window from the backend, so there is no
      // state to reset afterwards; a failure leaves the popup up to explain itself.
      const done = item.kind === "emoji" ? quickCommitText(item.label) : quickCommitClip(item.id);
      done.catch((e) => {
        setError(errorMessage(e));
        setBusy(false);
      });
    },
    [rows, busy],
  );

  /** Down: the next older row, or the request for the ten most recent first. */
  const older = useCallback(() => {
    if (searching) {
      setSelected((current) => Math.min(current + 1, rows.length - 1));
      return;
    }
    if (!expanded) {
      // The highlight stays on the newest row, so the press that opens the list
      // cannot also commit whatever it opened onto.
      setExpanded(true);
      return;
    }
    setSelected((current) => Math.min(current + 1, rows.length - 1));
  }, [searching, expanded, rows.length]);

  /** Up: the next newer row, and closing the list once the top is reached. */
  const newer = useCallback(() => {
    if (selected > 0) {
      setSelected((current) => current - 1);
    } else if (expanded) {
      setExpanded(false);
    }
  }, [selected, expanded]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        dismiss();
      } else if (event.key === "ArrowDown") {
        event.preventDefault();
        older();
      } else if (event.key === "ArrowUp") {
        event.preventDefault();
        newer();
      } else if (event.key === "Enter") {
        event.preventDefault();
        commit(selected);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [dismiss, older, newer, commit, selected]);

  // Clicking away is a dismissal. A webview that is created hidden can report a
  // blur before it has ever reported the focus it was given, so an early blur is
  // only believed once the window has actually had the focus (or has been up long
  // enough that no such event is still in flight).
  const sawFocus = useRef(false);
  const mountedAt = useRef(Date.now());
  useEffect(() => {
    let stop: (() => void) | undefined;
    currentWindow()
      ?.onFocusChanged(({ payload }) => {
        if (payload) {
          sawFocus.current = true;
          return;
        }
        if (sawFocus.current || Date.now() - mountedAt.current > 500) dismiss();
      })
      .then((unlisten) => {
        stop = unlisten;
      })
      .catch(() => undefined);
    return () => stop?.();
  }, [dismiss]);

  useEffect(() => {
    inputRef.current?.focus();
  }, []);

  const hint = searching
    ? `${rows.length} match${rows.length === 1 ? "" : "es"}`
    : expanded
      ? `${rows.length} recent · Up collapses`
      : "Down for more · Esc closes";

  return (
    <div className="flex h-full flex-col overflow-hidden rounded-panel border border-edge bg-panel">
      <div className="flex shrink-0 items-center gap-2 border-b border-edge-soft px-2">
        <input
          ref={inputRef}
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          spellCheck={false}
          placeholder="Search Kept…"
          aria-label="Search Kept"
          style={{ height: HEADER_HEIGHT }}
          className="min-w-0 flex-1 bg-transparent text-[12px] text-ink placeholder:text-ink-faint focus:outline-none"
        />
        {busy && <span className="shrink-0 text-[10px] text-ink-faint">pasting…</span>}
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto">
        {rows.length === 0 ? (
          <p className="px-2 py-2 text-[11px] text-ink-faint">
            {searching ? "No matches in Kept." : "Nothing copied yet."}
          </p>
        ) : (
          <div role="listbox" aria-label="Kept matches">
            {rows.map((item, index) => (
              <Row
                key={`${item.kind}-${item.id}-${index}`}
                item={item}
                active={index === selected}
                onCommit={() => commit(index)}
                onHover={() => setSelected(index)}
              />
            ))}
          </div>
        )}
      </div>

      <div
        className="shrink-0 border-t border-edge-soft px-2 text-[10px] leading-tight"
        style={{ minHeight: FOOTER_HEIGHT }}
      >
        {error ? (
          <span role="alert" className="text-danger">
            {error}
          </span>
        ) : (
          <span className="text-ink-faint">
            {hint} · Enter pastes into the app you were typing in
          </span>
        )}
      </div>
    </div>
  );
}
