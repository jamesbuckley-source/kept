import { useCallback, useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";

import { errorMessage } from "../lib/ipc";
import { IconClose } from "./icons";
import { KeptMark } from "./KeptMark";
import { ReleaseNotesFrame } from "./ReleaseNotesFrame";

/**
 * The update window: the release notes as an embedded page, with the offer to
 * install, skip this version, or be reminded next time.
 *
 * The window is opened by the launch check in the main window, so it asks the
 * updater for the release itself. Notes are handed to the embedded page as a
 * message rather than baked into the URL, which keeps the page a plain HTML file
 * that also opens in a browser.
 */
type UpdateState =
  | { kind: "checking" }
  | { kind: "available"; version: string; notes: string | null }
  | { kind: "up-to-date" }
  | { kind: "downloading"; version: string; percent: number | null }
  | { kind: "error"; message: string };

export function WhatsNewWindow({ onSkip }: { onSkip: (version: string) => void }) {
  const [state, setState] = useState<UpdateState>({ kind: "checking" });
  const pending = useRef<Update | null>(null);

  useEffect(() => {
    let cancelled = false;
    check()
      .then((found) => {
        if (cancelled) return;
        if (!found) {
          setState({ kind: "up-to-date" });
          return;
        }
        pending.current = found;
        setState({ kind: "available", version: found.version, notes: found.body ?? null });
      })
      .catch((e) => {
        if (cancelled) return;
        setState({ kind: "error", message: errorMessage(e) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const close = useCallback(() => {
    void getCurrentWindow().close();
  }, []);

  // Escape belongs to the window, like every other Kept window.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      close();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [close]);

  const install = useCallback(async () => {
    const found = pending.current;
    if (!found) return;
    const version = found.version;
    setState({ kind: "downloading", version, percent: null });
    try {
      let total = 0;
      let received = 0;
      await found.downloadAndInstall((event) => {
        if (event.event === "Started") {
          total = event.data.contentLength ?? 0;
        } else if (event.event === "Progress") {
          received += event.data.chunkLength;
          const percent = total > 0 ? Math.min(100, Math.round((received / total) * 100)) : null;
          setState({ kind: "downloading", version, percent });
        }
      });
      await relaunch();
    } catch (e) {
      setState({
        kind: "error",
        message: `The update could not be installed: ${errorMessage(e)}`,
      });
    }
  }, []);

  const available = state.kind === "available" || state.kind === "downloading";
  const version = state.kind === "available" ? state.version : null;
  const notes = state.kind === "available" ? state.notes : null;

  return (
    <div className="flex h-screen flex-col bg-canvas text-ink">
      <header
        data-drag-window
        className="flex shrink-0 items-center gap-2.5 border-b border-edge bg-panel px-3 py-2"
      >
        <KeptMark className="h-5 w-auto shrink-0 text-ink" />
        <h1 data-drag-window className="text-[12px] font-semibold tracking-wide">
          What&apos;s new
        </h1>
        <span data-drag-window className="min-w-0 flex-1 truncate text-[11px] text-ink-faint">
          {statusLine(state)}
        </span>
        <button
          type="button"
          onClick={close}
          title="Close (Esc)"
          aria-label="Close"
          className="flex size-6 items-center justify-center rounded text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink"
        >
          <IconClose className="size-3.5" />
        </button>
      </header>

      <ReleaseNotesFrame
        version={version}
        notes={notes}
        className="min-h-0 flex-1 border-0 bg-canvas"
      />

      <footer className="flex shrink-0 items-center gap-2 border-t border-edge bg-panel px-3 py-2">
        {state.kind === "downloading" && (
          <span className="flex items-center gap-1.5 text-[10px] text-ink-dim">
            <span className="h-1.5 w-20 overflow-hidden rounded-full bg-panel-3">
              <span
                className="block h-full rounded-full bg-accent transition-[width] duration-200"
                style={{ width: `${state.percent ?? 5}%` }}
              />
            </span>
            {state.percent !== null ? `${state.percent}%` : "Starting…"}
          </span>
        )}

        <span className="min-w-0 flex-1 truncate text-[10px] text-ink-faint">
          {state.kind === "error" ? state.message : ""}
        </span>

        <button
          type="button"
          onClick={close}
          className="rounded border border-edge bg-panel-2 px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink"
        >
          Remind me later
        </button>
        <button
          type="button"
          onClick={() => {
            if (version) onSkip(version);
            close();
          }}
          disabled={!version}
          title="Do not offer this version again"
          className="rounded border border-edge bg-panel-2 px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink disabled:opacity-40"
        >
          Skip this version
        </button>
        <button
          type="button"
          onClick={() => void install()}
          disabled={!available || state.kind === "downloading"}
          className="rounded border border-accent-dim bg-accent/20 px-2.5 py-1 text-[11px] font-medium text-accent-hi transition-colors hover:bg-accent/30 disabled:opacity-40"
        >
          {state.kind === "downloading" ? "Installing…" : "Update now"}
        </button>
      </footer>
    </div>
  );
}

/** The one line beside the title that says where the update stands. */
function statusLine(state: UpdateState): string {
  switch (state.kind) {
    case "checking":
      return "Checking for updates…";
    case "available":
      return `Kept ${state.version} is ready to install.`;
    case "up-to-date":
      return "This is the newest release.";
    case "downloading":
      return `Downloading Kept ${state.version}…`;
    case "error":
      return "The update could not be checked.";
  }
}
