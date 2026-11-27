/**
 * The paste stack in its own always-on-top window.
 *
 * The queue is only useful if opening it does not hide the application being
 * pasted into, so it lives in a small frameless window that can be parked beside
 * the editor. Files dropped on the window are imported and queued, which is how a
 * screenshot dragged out of Explorer joins the stack without a detour through the
 * history list.
 */

import { useState } from "react";

import { clipImageUrl, clipLabel, clipMotionUrl } from "../lib/format";
import { errorMessage, mediaUrl } from "../lib/ipc";
import type { StackCopyOutcome, StackEntry } from "../lib/types";
import { AnimatedImage } from "./AnimatedImage";
import { ColorSwatch } from "./ColorSwatch";
import {
  IconClose,
  IconCopy,
  IconImage,
  IconPaste,
  IconPasteAll,
  IconStack,
  IconText,
} from "./icons";

/** One queued item: a clip's thumbnail or a line from a split. */
function StackRow({
  entry,
  index,
  onRemove,
}: {
  entry: StackEntry;
  index: number;
  onRemove: () => void;
}) {
  const clip = entry.clip;
  const image = clip ? clipImageUrl(clip) : null;
  const label = clip ? clip.preview || clipLabel(clip) : (entry.text ?? "");

  return (
    <div className="group flex items-center gap-2 px-2 py-1.5 hover:bg-panel-2">
      <span className="w-4 shrink-0 text-right text-[10px] tabular-nums text-ink-faint">
        {index + 1}
      </span>
      {clip?.color ? (
        <ColorSwatch
          color={clip.color}
          label={clip.color}
          className="h-5 w-7"
          textClassName="hidden"
        />
      ) : clip && image ? (
        <AnimatedImage
          still={clip.thumbFile ? mediaUrl(clip.thumbFile) : null}
          animated={clip.video ? null : clipMotionUrl(clip)}
          video={clip.video ? clipMotionUrl(clip) : null}
          className="h-5 w-7 shrink-0 rounded border border-edge-soft object-cover"
        />
      ) : (
        <span className="flex h-5 w-7 shrink-0 items-center justify-center rounded border border-edge-soft bg-panel-3 text-ink-faint">
          {clip ? <IconImage className="size-2.5" /> : <IconText className="size-2.5" />}
        </span>
      )}
      <span className="min-w-0 flex-1 truncate text-[11px] text-ink" title={label}>
        {label || "(empty)"}
        {!clip && <span className="ml-1 text-[9px] uppercase text-ink-faint">line</span>}
      </span>
      <button
        type="button"
        onClick={onRemove}
        title="Remove from the stack"
        aria-label="Remove from the stack"
        className="shrink-0 rounded p-0.5 text-ink-faint opacity-0 transition-opacity hover:text-danger group-hover:opacity-100 focus-visible:opacity-100"
      >
        <IconClose className="size-3" />
      </button>
    </div>
  );
}

/** The plain-language result of "Copy all" and "Paste all". */
function copyAllMessage(outcome: StackCopyOutcome): string {
  const items = `${outcome.items} item${outcome.items === 1 ? "" : "s"}`;
  const images =
    outcome.images > 0
      ? `, ${outcome.images} image${outcome.images === 1 ? "" : "s"}`
      : "";
  const skipped =
    outcome.skipped > 0
      ? ` — ${outcome.skipped} image${outcome.skipped === 1 ? "" : "s"} kept as a path`
      : "";
  return `Copied ${items}${images}${skipped}.`;
}

function pasteAllMessage(outcome: StackCopyOutcome): string {
  const items = `${outcome.items} item${outcome.items === 1 ? "" : "s"}`;
  const images =
    outcome.images > 0
      ? `, ${outcome.images} image${outcome.images === 1 ? "" : "s"}`
      : "";
  const skipped = outcome.skipped > 0 ? `, ${outcome.skipped} skipped` : "";
  return `Pasted ${items}${images}${skipped}.`;
}

/** One of the queue actions: an icon, a name for a reader, and a tooltip. */
function ActionButton({
  label,
  hint,
  onClick,
  disabled,
  primary,
  busy,
  children,
}: {
  label: string;
  hint: string;
  onClick: () => void;
  disabled: boolean;
  /** The action the panel expects a press of: paste the next item. */
  primary?: boolean;
  busy?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={`${label} — ${hint}`}
      aria-label={label}
      aria-busy={busy || undefined}
      className={
        primary
          ? "flex h-7 flex-1 items-center justify-center gap-1.5 rounded bg-accent/20 px-2 text-[11px] font-medium text-accent-hi transition-colors hover:bg-accent/30 disabled:opacity-50"
          : "flex size-7 shrink-0 items-center justify-center rounded border border-edge bg-panel-2 text-ink-dim transition-colors hover:border-accent-dim hover:text-ink disabled:opacity-50"
      }
    >
      <span className={busy ? "animate-pulse" : undefined}>{children}</span>
      {primary && <span className="truncate">{label}</span>}
    </button>
  );
}

/** Numbered rows, the empty state, and the three actions with their shortcut. */
function StackBody({
  items,
  shortcut,
  onPasteFirst,
  onPasteAll,
  onCopyAll,
  onRemove,
  status,
  className,
}: {
  items: StackEntry[];
  shortcut: string;
  onPasteFirst: () => void;
  /** Pastes every item, one at a time, in its own native shape. */
  onPasteAll: () => Promise<StackCopyOutcome>;
  /** Copies every item into one clipboard record. */
  onCopyAll: () => Promise<StackCopyOutcome>;
  onRemove: (id: number) => void;
  /** Something the window was told from outside, e.g. how a drop went. */
  status: string | null;
  /** The list's own sizing. */
  className: string;
}) {
  const [message, setMessage] = useState<string | null>(null);
  const [busy, setBusy] = useState<"copy" | "paste" | null>(null);

  const run = async (kind: "copy" | "paste") => {
    setBusy(kind);
    setMessage(null);
    try {
      const outcome = kind === "copy" ? await onCopyAll() : await onPasteAll();
      setMessage(
        kind === "copy" ? copyAllMessage(outcome) : pasteAllMessage(outcome),
      );
    } catch (e) {
      setMessage(errorMessage(e));
    } finally {
      setBusy(null);
    }
  };

  const line = message ?? status;

  return (
    <>
      {items.length === 0 ? (
        <p className={`px-3 py-4 text-[11px] leading-relaxed text-ink-faint ${className}`}>
          Nothing queued. Hover a clip and press the stack button, convert a collection,
          split a text list, or drop files here.
        </p>
      ) : (
        <div className={`overflow-y-auto py-1 ${className}`}>
          {items.map((entry, index) => (
            <StackRow
              key={entry.id}
              entry={entry}
              index={index}
              onRemove={() => onRemove(entry.id)}
            />
          ))}
        </div>
      )}

      <div className="shrink-0 border-t border-edge-soft px-2 py-1.5">
        <div className="mb-1 flex items-center justify-center gap-1.5">
          <ActionButton
            label="Paste next"
            hint={shortcut ? `${shortcut}, or the window you were last in` : "the window you were last in"}
            onClick={onPasteFirst}
            disabled={items.length === 0}
            primary
          >
            <IconPaste className="size-4" />
          </ActionButton>
          <ActionButton
            label="Paste all"
            hint="every item, one after another, each as its own picture or text"
            onClick={() => void run("paste")}
            disabled={items.length === 0 || busy !== null}
            busy={busy === "paste"}
          >
            <IconPasteAll className="size-4" />
          </ActionButton>
          <ActionButton
            label="Copy all"
            hint="every item into one clipboard record"
            onClick={() => void run("copy")}
            disabled={items.length === 0 || busy !== null}
            busy={busy === "copy"}
          >
            <IconCopy className="size-4" />
          </ActionButton>
        </div>
        {busy === "copy" ? (
          // The clipboard is written last, so a paste made while this is showing
          // lands on whatever was there before — worth saying out loud, because a
          // stack of screenshots takes a second to encode.
          <p role="status" className="text-[10px] leading-snug text-ink-faint">
            Building the record… the old clipboard stays until this finishes.
          </p>
        ) : line ? (
          <p role="status" className="text-[10px] leading-snug text-ink-dim">
            {line}
          </p>
        ) : (
          <p className="text-[10px] leading-snug text-ink-faint">
            {shortcut
              ? `${shortcut} pastes the next item, in order`
              : "No shortcut set"}
            {" · each item is used up as it is pasted"}
          </p>
        )}
      </div>
    </>
  );
}

/** Count, Clear all, and the window's close button. */
function StackHeader({ count, onClear, onClose }: {
  count: number;
  onClear: () => void;
  onClose: () => void;
}) {
  // A frameless window moves by dragging, and only the element carrying this
  // attribute is a handle — so it goes on the header *and* its text, or the title
  // and the icon are dead spots.
  return (
    <div
      data-drag-window
      className="flex shrink-0 items-center gap-1.5 border-b border-edge-soft px-2 py-1.5"
    >
      <span data-drag-window className="shrink-0 text-accent">
        <IconStack className="size-3.5" />
      </span>
      <span data-drag-window className="min-w-0 flex-1 truncate text-[11px] font-semibold text-ink">
        Stacks ({count})
      </span>
      <button
        type="button"
        onClick={onClear}
        title="Remove every queued item"
        className="shrink-0 rounded px-1.5 py-0.5 text-[10px] text-ink-faint transition-colors hover:bg-panel-3 hover:text-danger"
      >
        Clear all
      </button>
      <button
        type="button"
        onClick={onClose}
        title="Close this window"
        aria-label="Close this window"
        className="shrink-0 rounded p-1 text-ink-faint transition-colors hover:bg-panel-3 hover:text-ink"
      >
        <IconClose className="size-3.5" />
      </button>
    </div>
  );
}

/**
 * The queue in its own window, which the backend keeps above other applications.
 * Closing it closes it — the queue is still there for the next time it is asked
 * for.
 */
export function PasteStackWindow({
  items,
  shortcut,
  status,
  dropping,
  onClose,
  onPasteFirst,
  onPasteAll,
  onCopyAll,
  onRemove,
  onClear,
}: {
  items: StackEntry[];
  shortcut: string;
  /** How a drop or a failure went, said in the status line. */
  status: string | null;
  /** True while files are hovering over the window. */
  dropping: boolean;
  onClose: () => void;
  onPasteFirst: () => void;
  onPasteAll: () => Promise<StackCopyOutcome>;
  onCopyAll: () => Promise<StackCopyOutcome>;
  onRemove: (id: number) => void;
  onClear: () => void;
}) {
  return (
    <div className="relative flex h-full flex-col overflow-hidden border border-edge bg-panel">
      <StackHeader count={items.length} onClear={onClear} onClose={onClose} />
      <StackBody
        items={items}
        shortcut={shortcut}
        onPasteFirst={onPasteFirst}
        onPasteAll={onPasteAll}
        onCopyAll={onCopyAll}
        onRemove={onRemove}
        status={status}
        className="min-h-0 flex-1"
      />
      {dropping && (
        <div className="pointer-events-none absolute inset-0 z-10 flex items-center justify-center border-2 border-dashed border-accent bg-panel/85 text-[11px] font-medium text-accent-hi">
          Drop to queue
        </div>
      )}
    </div>
  );
}