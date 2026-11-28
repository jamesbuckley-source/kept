/**
 * The release notes that ship inside the app.
 *
 * `docs/release-notes.md` is the file a release publishes as its GitHub body, so
 * bundling it means "Show latest updates" always has the real notes for the build
 * that is running — not a sample — even when no update is waiting and the machine
 * is offline.
 */

import bundled from "../../docs/release-notes.md?raw";

export const bundledReleaseNotes: string = bundled;

/**
 * The headlines a release body announces, as plain one-liners.
 *
 * A notes file is headings with details under them, so the headings are the list;
 * GitHub's auto-generated notes have no headings and are all bullets, which is why
 * those are the fallback. Returns at most `count` lines.
 */
export function notesHighlights(notes: string | null | undefined, count: number): string[] {
  if (!notes) return [];
  const headings: string[] = [];
  const bullets: string[] = [];
  // A notes file is a page or two; the cap only stops a pathological body.
  const limit = 60;
  for (const raw of notes.split(/\r?\n/)) {
    const line = raw.trim();
    // HTML comments annotate the file for the people writing it, not the reader.
    if (!line || line.startsWith("<!--")) continue;
    const heading = /^#{1,4}\s+(.*)$/.exec(line);
    const bullet = /^[-*]\s+(.*)$/.exec(line);
    if (heading) push(headings, heading[1], limit);
    else if (bullet) push(bullets, bullet[1], limit);
  }
  const chosen = headings.length > 0 ? headings : bullets;
  return chosen.slice(0, count);
}

function push(into: string[], line: string, count: number) {
  if (into.length >= count) return;
  const text = plain(line);
  if (text && !into.includes(text)) into.push(text);
}

/** Markdown's links, emphasis and code marks, reduced to their words. */
function plain(text: string): string {
  return text
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/[*`_]/g, "")
    .trim();
}
