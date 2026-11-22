import { useCallback, useEffect, useRef } from "react";

import { openUrl } from "../lib/ipc";

interface ReleaseNotesFrameProps {
  /** The version the notes describe; null while it is still being checked for. */
  version: string | null;
  /** The markdown body of the release, or null to leave the page's examples. */
  notes: string | null;
  className?: string;
}

/**
 * The release-notes page, embedded.
 *
 * The page is a plain HTML file (`public/release-notes.html`) so it also opens in
 * a browser; here it is handed the version and the release body by message, and
 * sends links back out through the app rather than navigating the frame.
 */
export function ReleaseNotesFrame({ version, notes, className }: ReleaseNotesFrameProps) {
  const frame = useRef<HTMLIFrameElement | null>(null);

  const publish = useCallback(() => {
    const target = frame.current?.contentWindow;
    if (!target || !version) return;
    target.postMessage({ type: "kept:release-notes", version, notes }, "*");
  }, [notes, version]);

  // Either side can be ready first: the page announces itself, and this fires
  // again whenever the release arrives.
  useEffect(() => {
    publish();
  }, [publish]);

  useEffect(() => {
    const onMessage = (event: MessageEvent) => {
      const data = event.data as { type?: string; url?: string } | null;
      if (data?.type === "kept:ready") {
        publish();
      } else if (data?.type === "kept:open-url" && data.url) {
        void openUrl(data.url).catch(() => undefined);
      }
    };
    window.addEventListener("message", onMessage);
    return () => window.removeEventListener("message", onMessage);
  }, [publish]);

  return (
    <iframe
      ref={frame}
      src="/release-notes.html"
      title="Release notes"
      onLoad={publish}
      className={className}
    />
  );
}
