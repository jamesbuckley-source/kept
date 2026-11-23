/**
 * The label / hint / control row every settings list is built from.
 *
 * It lives on its own so a component that both shows rows and is used *by* the
 * settings panel — the macOS permission grants — can borrow the layout without
 * importing the panel that renders it.
 */

import type { ReactNode } from "react";

export function Row({
  label,
  hint,
  control,
}: {
  label: string;
  hint?: string;
  control: ReactNode;
}) {
  return (
    <div className="flex items-start gap-4 py-2">
      <div className="min-w-0 flex-1">
        <p className="text-[12px] text-ink-dim">{label}</p>
        {hint && <p className="mt-0.5 text-[10px] leading-relaxed text-ink-faint">{hint}</p>}
      </div>
      <div className="shrink-0 pt-px">{control}</div>
    </div>
  );
}
