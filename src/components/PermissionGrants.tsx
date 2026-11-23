/**
 * The system grants macOS keeps for an app, with the button that asks for each.
 *
 * Both live in System Settings rather than in Kept's own settings, so the only
 * thing this can do is say whether they are held and take the user to the pane
 * that changes them. macOS applies an answer without restarting the app, so the
 * state is re-read after the button is pressed: a panel that never refreshed
 * would keep saying "not granted" after the user had already allowed it.
 *
 * Renders nothing off macOS, and nothing while the state is still being read, so
 * neither the wizard nor Settings shows an empty block on Windows or Linux.
 */

import { useEffect, useState } from "react";

import { permissions, requestPermission, type MacosPermissions } from "../lib/ipc";
import { isMac } from "../lib/platform";
import { Row } from "./SettingsRow";

export function PermissionGrants({ compact = false }: { compact?: boolean }) {
  const [grants, setGrants] = useState<MacosPermissions | null>(null);

  useEffect(() => {
    if (!isMac) return;
    let live = true;
    permissions()
      .then((next) => {
        if (live) setGrants(next);
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  if (!isMac || !grants?.macos) return null;

  const allow = (grant: "screenRecording" | "accessibility") => {
    requestPermission(grant)
      .then(setGrants)
      .catch(() => {});
  };

  return (
    <div className={compact ? undefined : "py-2"}>
      {!compact && (
        <>
          <p className="text-[12px] text-ink-dim">macOS permissions</p>
          <p className="mt-0.5 text-[10px] leading-relaxed text-ink-faint">
            macOS only lets an app use these after you say so. Both can be changed later in
            System Settings → Privacy &amp; Security.
          </p>
        </>
      )}
      <div className={compact ? undefined : "mt-1.5 flex flex-col gap-1.5"}>
        <Row
          label="Screen Recording"
          hint="Screenshots, the colour picker and screen recording read the screen."
          control={
            <GrantButton
              granted={grants.screenRecording}
              label="Screen Recording"
              onAllow={() => allow("screenRecording")}
            />
          }
        />
        <Row
          label="Accessibility"
          hint="Pasting for you and typing :: in another app press keys on your behalf."
          control={
            <GrantButton
              granted={grants.accessibility}
              label="Accessibility"
              onAllow={() => allow("accessibility")}
            />
          }
        />
      </div>
    </div>
  );
}

function GrantButton({
  granted,
  label,
  onAllow,
}: {
  granted: boolean;
  label: string;
  onAllow: () => void;
}) {
  if (granted) {
    return <span className="text-[10px] text-ink-faint">Granted</span>;
  }
  return (
    <button
      type="button"
      onClick={onAllow}
      className="rounded border border-edge bg-panel-2 px-2 py-1 text-[10px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink"
    >
      Allow {label}…
    </button>
  );
}
