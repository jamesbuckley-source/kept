import { useCallback, useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { useSettings } from "../hooks/useSettings";
import { FirstRunWizard } from "./FirstRunWizard";

/**
 * The setup wizard in a window of its own.
 *
 * It used to be a full-screen overlay inside the history window: a modal that could
 * not be moved off the clips it was asking about, with no title bar of its own. The
 * page is the same form; only the frame around it changed.
 */
export function WizardWindow() {
  const { settings, error, patch } = useSettings();

  const close = useCallback(() => {
    void getCurrentWindow().close().catch(() => undefined);
  }, []);

  // Closing the window on a first run is the same answer as Skip: without this the
  // wizard would be waiting again on the next launch.
  useEffect(() => {
    if (!settings || settings.setupComplete) return;
    const win = getCurrentWindow();
    let handled = false;
    const pending = win.onCloseRequested(async (event) => {
      if (handled) return;
      handled = true;
      event.preventDefault();
      try {
        await patch({ setupComplete: true });
      } catch {
        // A failed write must not trap anyone in the window.
      }
      await win.close();
    });
    return () => {
      void pending.then((stop) => stop()).catch(() => undefined);
    };
  }, [settings, patch]);

  if (!settings) {
    return (
      <div className="flex h-screen w-screen items-center justify-center bg-panel">
        <p className="text-[12px] text-ink-faint">{error ?? "Loading settings…"}</p>
      </div>
    );
  }

  return (
    <FirstRunWizard
      settings={settings}
      firstRun={!settings.setupComplete}
      onApply={patch}
      onClose={close}
      onFinish={close}
    />
  );
}
