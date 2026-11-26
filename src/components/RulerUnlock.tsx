/**
 * The small chip that stays clickable while the ruler is locked.
 *
 * A locked ruler is click-through so the desktop underneath can be used, which
 * also means it can no longer see a click to unlock itself. Esc is grabbed
 * system-wide on X11, but Wayland does not deliver that grab, so this window is
 * the door back that works on both.
 */

import { useEffect } from "react";

import { errorMessage, rulerLock } from "../lib/ipc";

export function RulerUnlock() {
  useEffect(() => {
    const { body, documentElement } = document;
    const previous = [body.style.background, documentElement.style.background];
    body.style.background = "transparent";
    documentElement.style.background = "transparent";
    return () => {
      body.style.background = previous[0];
      documentElement.style.background = previous[1];
    };
  }, []);

  return (
    <div className="fixed inset-0 flex items-center justify-center">
      <button
        type="button"
        onClick={() => {
          void rulerLock(false).catch((error) => console.error(errorMessage(error)));
        }}
        className="rounded-full border border-accent bg-panel px-5 py-2.5 text-[12px] font-medium text-ink shadow-xl transition-colors hover:bg-panel-2 focus:outline-none"
      >
        Unlock ruler
      </button>
    </div>
  );
}
