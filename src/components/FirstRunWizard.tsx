import { useEffect, useState, type ReactNode } from "react";

import { errorMessage, detectKeptCraft, validateHotkey } from "../lib/ipc";
import { isMac } from "../lib/platform";
import type { Settings, ThemeMode } from "../lib/types";
import { CaptureModeField } from "./CaptureModeField";
import { PermissionGrants } from "./PermissionGrants";
import { HotkeyField, Row, Toggle } from "./SettingsPanel";
import { IconChevronLeft, IconClose } from "./icons";

/** Process names the wizard offers to keep out of history. */
const PASSWORD_MANAGERS = [
  "keepassxc.exe",
  "bitwarden.exe",
  "1password.exe",
  "dashlane.exe",
];

type Retention =
  | "500"
  | "2000"
  | "1d"
  | "30d"
  | "90d"
  | "forever"
  | "custom-clips"
  | "custom-days";
type SecretMode = "flag" | "skip" | "off";

/** The presets that need no number typed in; the two custom kinds do. */
type FixedRetention = Exclude<Retention, "custom-clips" | "custom-days">;

/** Mirrors the backend's caps, so a typed number is never silently reduced. */
const MAX_CLIPS = 100_000;
const MAX_DAYS = 3_650;

const RETENTION_PATCH: Record<
  FixedRetention,
  Pick<Settings, "maxItems" | "maxAgeDays" | "maxAgeHours">
> = {
  "500": { maxItems: 500, maxAgeDays: 0, maxAgeHours: 0 },
  "2000": { maxItems: 2000, maxAgeDays: 0, maxAgeHours: 0 },
  "1d": { maxItems: 0, maxAgeDays: 1, maxAgeHours: 0 },
  "30d": { maxItems: 0, maxAgeDays: 30, maxAgeHours: 0 },
  "90d": { maxItems: 0, maxAgeDays: 90, maxAgeHours: 0 },
  forever: { maxItems: 0, maxAgeDays: 0, maxAgeHours: 0 },
};

const RETENTION_LABELS: { value: Retention; label: string }[] = [
  { value: "500", label: "500 clips" },
  { value: "2000", label: "2,000 clips" },
  { value: "1d", label: "1 day" },
  { value: "30d", label: "30 days" },
  { value: "90d", label: "90 days" },
  { value: "forever", label: "Forever" },
  { value: "custom-clips", label: "Custom clips" },
  { value: "custom-days", label: "Custom days" },
];

/** Secret expiry, in minutes; 0 is "never", which is the default. */
const SECRET_EXPIRY: { value: number; label: string }[] = [
  { value: 0, label: "Never" },
  { value: 15, label: "15 min" },
  { value: 60, label: "1 hour" },
  { value: 480, label: "8 hours" },
  { value: 1440, label: "1 day" },
];

const SECRET_OPTIONS: { value: SecretMode; label: string; hint: string }[] = [
  {
    value: "flag",
    label: "Flag and blur",
    hint: "Detected credentials stay in history behind a Secret label.",
  },
  {
    value: "skip",
    label: "Leave out of history",
    hint: "Flagged copies are never stored at all.",
  },
  {
    value: "off",
    label: "Do not detect",
    hint: "Secrets are captured like any other text.",
  },
];

function initialRetention(settings: Settings): Retention {
  if (settings.maxAgeDays === 1) return "1d";
  if (settings.maxAgeDays === 30) return "30d";
  if (settings.maxAgeDays === 90) return "90d";
  if (settings.maxAgeDays === 0 && settings.maxAgeHours === 0) {
    if (settings.maxItems === 500) return "500";
    if (settings.maxItems === 2000) return "2000";
    if (settings.maxItems === 0) return "forever";
    return "custom-clips";
  }
  return "custom-days";
}

/** A custom limit's starting point: what is stored, or a round number to edit. */
function initialCustomClips(settings: Settings): string {
  return String(settings.maxItems > 0 ? settings.maxItems : 1000);
}

function initialCustomDays(settings: Settings): string {
  if (settings.maxAgeDays > 0) return String(settings.maxAgeDays);
  if (settings.maxAgeHours > 0) return String(Math.max(1, Math.round(settings.maxAgeHours / 24)));
  return "7";
}

function initialSecretMode(settings: Settings): SecretMode {
  if (!settings.flagSecrets) return "off";
  return settings.skipSecretCapture ? "skip" : "flag";
}

function ignoresPasswordManagers(settings: Settings): boolean {
  return PASSWORD_MANAGERS.every((app) => settings.ignoredApps.includes(app));
}

function initialThemeMode(settings: Settings): ThemeMode {
  return settings.theme.mode === "light" ? "light" : "dark";
}

interface FirstRunWizardProps {
  settings: Settings;
  /** True when the wizard opens automatically, false when re-run from Settings. */
  firstRun: boolean;
  onApply: (delta: Partial<Settings>) => Promise<Settings>;
  onClose: () => void;
  /** Saves the choices and closes the wizard. */
  onFinish: () => void;
}

function OptionButton({
  selected,
  onClick,
  children,
}: {
  selected: boolean;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      aria-pressed={selected}
      onClick={onClick}
      className={`rounded border px-2.5 py-1 text-[11px] transition-colors ${
        selected
          ? "border-accent bg-accent/20 text-accent-hi"
          : "border-edge bg-panel-2 text-ink-dim hover:bg-panel-3 hover:text-ink"
      }`}
    >
      {children}
    </button>
  );
}

function StepTitle({ title, hint }: { title: string; hint: string }) {
  return (
    <div className="mb-3">
      <h2 className="text-[13px] font-semibold text-ink">{title}</h2>
      <p className="mt-0.5 text-[11px] leading-relaxed text-ink-faint">{hint}</p>
    </div>
  );
}

export function FirstRunWizard({
  settings,
  firstRun,
  onApply,
  onClose,
  onFinish,
}: FirstRunWizardProps) {
  const [step, setStep] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const [hotkey, setHotkey] = useState(settings.hotkey);
  const [newClipHotkey, setNewClipHotkey] = useState(settings.newClipHotkey);
  const [launchAtLogin, setLaunchAtLogin] = useState(
    firstRun ? true : settings.launchAtLogin,
  );
  const [printScreen, setPrintScreen] = useState(settings.printScreenCapture);
  const [captureMode, setCaptureMode] = useState(settings.captureMode);
  const [autoPaste, setAutoPaste] = useState(firstRun ? true : settings.autoPaste);
  const [captureMedia, setCaptureMedia] = useState(
    settings.captureImages || settings.captureFiles,
  );
  const [ignoreManagers, setIgnoreManagers] = useState(
    firstRun ? true : ignoresPasswordManagers(settings),
  );
  // Left as the setting says, so a first run does not switch on a system-wide
  // keyboard hook behind the user's back: the wizard offers it, it does not assume.
  const [quickPaste, setQuickPaste] = useState(settings.quickPasteEnabled);
  const [secretMode, setSecretMode] = useState<SecretMode>(initialSecretMode(settings));
  const [secretExpiry, setSecretExpiry] = useState(settings.secretMaxAgeMins);
  const [retention, setRetention] = useState<Retention>(initialRetention(settings));
  const [customClips, setCustomClips] = useState(() => initialCustomClips(settings));
  const [customDays, setCustomDays] = useState(() => initialCustomDays(settings));
  const [themeMode, setThemeMode] = useState<ThemeMode>(initialThemeMode(settings));
  const [copySound, setCopySound] = useState(settings.copySound);
  const [failureSound, setFailureSound] = useState(settings.copyFailSound);
  /**
   * The command line KeptCraft was found at, or null when it is not installed.
   * Detection is a filesystem probe on the backend, so it runs once when the
   * wizard opens rather than on every step.
   */
  const [keptCraft, setKeptCraft] = useState<string | null>(null);
  const [useKeptCraft, setUseKeptCraft] = useState(false);
  useEffect(() => {
    let cancelled = false;
    detectKeptCraft()
      .then((command) => {
        if (cancelled || !command) return;
        setKeptCraft(command);
        // An editor the user has already chosen is left alone; an empty slot is
        // what this offer exists for.
        setUseKeptCraft(settings.imageEditor.trim() === "");
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
    // Deliberately once: the answer cannot change while the wizard is open.
  }, []);

  const initialTheme = initialThemeMode(settings);
  // Re-running the wizard must not normalise choices the user made in Settings:
  // only fields the user actually touched are written back.
  const initialCapture = settings.captureImages || settings.captureFiles;
  const initialIgnore = firstRun ? true : ignoresPasswordManagers(settings);
  const initialQuickPaste = settings.quickPasteEnabled;
  const initialRetentionValue = initialRetention(settings);

  const skip = async () => {
    if (!firstRun) {
      onClose();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onApply({ setupComplete: true });
      onClose();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      if (!busy) void skip();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [busy, firstRun, onApply, onClose]);

  const back = () => {
    setError(null);
    setStep((current) => Math.max(0, current - 1));
  };

  const advance = async () => {
    setError(null);
    if (step === 1) {
      setStep(2);
      return;
    }
    if (step !== 0) return;

    setBusy(true);
    try {
      const trimmed = hotkey.trim();
      if (trimmed && trimmed !== settings.hotkey) {
        await validateHotkey(trimmed);
      }
      const extra = newClipHotkey.trim();
      if (extra && extra !== settings.newClipHotkey) {
        await validateHotkey(extra);
      }
      // Registering the summon shortcut is the only way to know the OS will
      // accept it, so apply it before moving on instead of failing at the end.
      await onApply({
        hotkey: trimmed || settings.hotkey,
        newClipHotkey: extra,
        launchAtLogin,
        captureMode,
        printScreenCapture: printScreen,
      });
      setStep(1);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const finish = async () => {
    const clipsWanted = Number(customClips);
    const daysWanted = Number(customDays);
    if (
      retention === "custom-clips" &&
      !(Number.isInteger(clipsWanted) && clipsWanted >= 1 && clipsWanted <= MAX_CLIPS)
    ) {
      setError(`A custom clip limit must be a whole number from 1 to ${MAX_CLIPS}.`);
      return;
    }
    if (
      retention === "custom-days" &&
      !(Number.isInteger(daysWanted) && daysWanted >= 1 && daysWanted <= MAX_DAYS)
    ) {
      setError(`A custom number of days must be a whole number from 1 to ${MAX_DAYS}.`);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const delta: Partial<Settings> = {
        setupComplete: true,
        autoPaste,
        flagSecrets: secretMode !== "off",
        skipSecretCapture: secretMode === "skip",
        copySound,
        copyFailSound: failureSound,
        secretMaxAgeMins: secretExpiry,
      };

      if (firstRun || captureMedia !== initialCapture) {
        delta.captureImages = captureMedia;
        delta.captureFiles = captureMedia;
      }

      if (firstRun || ignoreManagers !== initialIgnore) {
        delta.ignoredApps = ignoreManagers
          ? Array.from(new Set([...settings.ignoredApps, ...PASSWORD_MANAGERS]))
          : settings.ignoredApps.filter((app) => !PASSWORD_MANAGERS.includes(app));
      }

      if (firstRun || quickPaste !== initialQuickPaste) {
        delta.quickPasteEnabled = quickPaste;
      }

      if (firstRun || retention !== initialRetentionValue) {
        if (retention === "custom-clips") {
          delta.maxItems = clipsWanted;
          delta.maxAgeDays = 0;
          delta.maxAgeHours = 0;
        } else if (retention === "custom-days") {
          delta.maxItems = 0;
          delta.maxAgeDays = daysWanted;
          delta.maxAgeHours = 0;
        } else {
          Object.assign(delta, RETENTION_PATCH[retention]);
        }
      }

      // Leave a custom palette alone when the wizard is only being re-run.
      if (firstRun || themeMode !== initialTheme) {
        delta.theme = { ...settings.theme, mode: themeMode };
      }

      // An editor already configured is never replaced by the offer above.
      if (useKeptCraft && keptCraft && settings.imageEditor !== keptCraft) {
        delta.imageEditor = keptCraft;
      }

      await onApply(delta);
      onFinish();
    } catch (e) {
      setError(errorMessage(e));
      setBusy(false);
    }
  };

  const steps = ["Activation", "Paste & privacy", "Look & finish"];

  return (
    <div className="flex h-screen w-screen bg-panel">
      <div
        role="dialog"
        aria-modal="true"
        aria-labelledby="setup-title"
        className="flex min-h-0 w-full flex-col bg-panel"
      >
        <div data-drag-window className="flex items-center gap-2 border-b border-edge-soft px-5 py-3">
          <div className="min-w-0 flex-1">
            <h2 id="setup-title" className="text-[13px] font-semibold text-ink">
              Set up Kept
            </h2>
            <p className="text-[10px] text-ink-faint">
              Three quick choices. Everything can be changed later in Settings.
            </p>
          </div>
          <button
            type="button"
            onClick={() => void skip()}
            disabled={busy}
            title={firstRun ? "Skip setup" : "Close"}
            aria-label={firstRun ? "Skip setup" : "Close"}
            className="flex size-6 shrink-0 items-center justify-center rounded text-ink-faint transition-colors hover:bg-panel-3 hover:text-ink disabled:opacity-40"
          >
            <IconClose className="size-3.5" />
          </button>
        </div>

        <div className="flex gap-1 px-5 pt-3" aria-hidden="true">
          {steps.map((label, index) => (
            <div
              key={label}
              className={`h-0.5 flex-1 rounded-full transition-colors ${
                index <= step ? "bg-accent" : "bg-panel-3"
              }`}
            />
          ))}
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-5 py-3">
          {error && (
            <p role="alert" className="mb-3 rounded border border-danger/40 bg-danger/10 px-2 py-1.5 text-[11px] text-danger">
              {error}
            </p>
          )}

          {step === 0 && (
            <>
              <StepTitle
                title="Activation"
                hint={
                  isMac
                    ? "The shortcut that summons the window from anywhere. It must be unique across macOS."
                    : "The shortcut that summons the window from anywhere. It must be unique across Windows."
                }
              />
              <Row
                label="Global hotkey"
                hint={
                  isMac
                    ? "Written with a plus sign, for example Super+Shift+V; Super is the Command key."
                    : "Written with a plus sign, for example Alt+V or Ctrl+Shift+V."
                }
                control={<HotkeyField value={settings.hotkey} onCommit={setHotkey} />}
              />
              <Row
                label="New clip shortcut"
                hint="Optional. Opens the New clip window from anywhere; empty means no shortcut."
                control={
                  <HotkeyField
                    optional
                    value={settings.newClipHotkey}
                    onCommit={setNewClipHotkey}
                  />
                }
              />
              <Row
                label={isMac ? "Start at login" : "Start with Windows"}
                hint="Runs quietly in the tray from sign-in and captures every copy."
                control={
                  <Toggle
                    label={isMac ? "Start at login" : "Start with Windows"}
                    checked={launchAtLogin}
                    onChange={setLaunchAtLogin}
                  />
                }
              />
              {/* Print Screen belongs to Windows: there is no such key on a Mac, so
                  the row would offer a setting that can never do anything. */}
              {!isMac && (
                <Row
                  label="Print Screen captures"
                  hint="Takes the key from the Snipping Tool and runs the default capture below. Turning it off later puts Windows back exactly as it was."
                  control={
                    <Toggle
                      label="Print Screen captures"
                      checked={printScreen}
                      onChange={setPrintScreen}
                    />
                  }
                />
              )}
              <Row
                label="Default capture"
                hint={
                  isMac
                    ? "What the header's capture button starts."
                    : "What Print Screen and the header's capture button start."
                }
                control={
                  <CaptureModeField
                    label="Default capture type"
                    value={captureMode}
                    onChange={setCaptureMode}
                  />
                }
              />
            </>
          )}

          {step === 1 && (
            <>
              <StepTitle
                title="Paste & privacy"
                hint="Choose what gets recorded and how clips come back out."
              />
              <PermissionGrants />
              <Row
                label="Paste automatically"
                hint={
                  isMac
                    ? "Sends Command+V to the window you were in after you pick a clip."
                    : "Sends Ctrl+V to the window you were in after you pick a clip."
                }
                control={
                  <Toggle
                    label="Paste automatically"
                    checked={autoPaste}
                    onChange={setAutoPaste}
                  />
                }
              />
              <Row
                label="Capture images and files"
                hint="Adds screenshots and copied File Explorer paths to history."
                control={
                  <Toggle
                    label="Capture images and files"
                    checked={captureMedia}
                    onChange={setCaptureMedia}
                  />
                }
              />
              <Row
                label="Ignore password managers"
                hint="Never records copies made in KeePassXC, Bitwarden, 1Password or Dashlane."
                control={
                  <Toggle
                    label="Ignore password managers"
                    checked={ignoreManagers}
                    onChange={setIgnoreManagers}
                  />
                }
              />
              <Row
                label="Type :: anywhere to search Kept"
                hint="Opens a small Kept search box while you write in another application. It watches what you type with a keyboard hook that compares your last few characters and keeps nothing; password managers and terminals are left out. Off by default."
                control={
                  <Toggle
                    label="Type :: anywhere to search Kept"
                    checked={quickPaste}
                    onChange={setQuickPaste}
                  />
                }
              />

              <div className="py-2">
                <p className="text-[12px] text-ink-dim">Likely secrets</p>
                <p className="mt-0.5 text-[10px] leading-relaxed text-ink-faint">
                  Detection stays on this machine.
                </p>
                <div className="mt-1.5 flex flex-col gap-1">
                  {SECRET_OPTIONS.map((option) => (
                    <button
                      key={option.value}
                      type="button"
                      aria-pressed={secretMode === option.value}
                      onClick={() => setSecretMode(option.value)}
                      className={`rounded border px-2.5 py-1.5 text-left transition-colors ${
                        secretMode === option.value
                          ? "border-accent bg-accent/15"
                          : "border-edge bg-panel-2 hover:bg-panel-3"
                      }`}
                    >
                      <span className="block text-[11px] text-ink">{option.label}</span>
                      <span className="block text-[10px] text-ink-faint">{option.hint}</span>
                    </button>
                  ))}
                </div>
              </div>

              <div className="py-2">
                <p className="text-[12px] text-ink-dim">Secrets expire after</p>
                <p className="mt-0.5 text-[10px] leading-relaxed text-ink-faint">
                  Pinning or filing a secret in a collection does not keep it past this.
                </p>
                <div className="mt-1.5 flex flex-wrap gap-1">
                  {SECRET_EXPIRY.map((option) => (
                    <OptionButton
                      key={option.value}
                      selected={secretExpiry === option.value}
                      onClick={() => setSecretExpiry(option.value)}
                    >
                      {option.label}
                    </OptionButton>
                  ))}
                </div>
              </div>

              <div className="py-2">
                <p className="text-[12px] text-ink-dim">Keep history for</p>
                <div className="mt-1.5 flex flex-wrap gap-1">
                  {RETENTION_LABELS.map((option) => (
                    <OptionButton
                      key={option.value}
                      selected={retention === option.value}
                      onClick={() => setRetention(option.value)}
                    >
                      {option.label}
                    </OptionButton>
                  ))}
                </div>
                {retention === "custom-clips" && (
                  <label className="mt-1.5 flex items-center gap-1.5 text-[10px] text-ink-faint">
                    Keep the newest
                    <input
                      type="number"
                      min={1}
                      max={MAX_CLIPS}
                      value={customClips}
                      onChange={(event) => setCustomClips(event.target.value)}
                      aria-label="Custom clip limit"
                      className="w-24 rounded border border-edge bg-panel-2 px-2 py-1 text-[11px] text-ink focus:border-accent-dim focus:outline-none"
                    />
                    clips
                  </label>
                )}
                {retention === "custom-days" && (
                  <label className="mt-1.5 flex items-center gap-1.5 text-[10px] text-ink-faint">
                    Forget clips after
                    <input
                      type="number"
                      min={1}
                      max={MAX_DAYS}
                      value={customDays}
                      onChange={(event) => setCustomDays(event.target.value)}
                      aria-label="Custom retention in days"
                      className="w-24 rounded border border-edge bg-panel-2 px-2 py-1 text-[11px] text-ink focus:border-accent-dim focus:outline-none"
                    />
                    days
                  </label>
                )}
              </div>
            </>
          )}

          {step === 2 && (
            <>
              <StepTitle
                title="Look & finish"
                hint="A theme, two sounds and the image editor, then you are done."
              />
              <Row
                label="Theme"
                hint="Custom colors stay available in Settings → Appearance."
                control={
                  <div className="flex items-center gap-0.5 rounded border border-edge bg-panel-2 p-0.5">
                    {(["dark", "light"] as const).map((mode) => (
                      <button
                        key={mode}
                        type="button"
                        aria-pressed={themeMode === mode}
                        onClick={() => setThemeMode(mode)}
                        className={`rounded px-2 py-0.5 text-[10px] capitalize transition-colors ${
                          themeMode === mode
                            ? "bg-accent/25 text-accent-hi"
                            : "text-ink-faint hover:text-ink-dim"
                        }`}
                      >
                        {mode}
                      </button>
                    ))}
                  </div>
                }
              />
              <Row
                label="Copy sound"
                hint="A short tick when a copy lands in history."
                control={
                  <Toggle label="Copy sound" checked={copySound} onChange={setCopySound} />
                }
              />
              <Row
                label="Failure sound"
                hint="A lower tone when a copy cannot be recorded."
                control={
                  <Toggle
                    label="Failure sound"
                    checked={failureSound}
                    onChange={setFailureSound}
                  />
                }
              />
              {keptCraft && (
                <Row
                  label="Edit images in KeptCraft"
                  hint="KeptCraft is installed. The Edit buttons on image clips will open it, saving the edit back over the clip."
                  control={
                    <Toggle
                      label="Edit images in KeptCraft"
                      checked={useKeptCraft}
                      onChange={setUseKeptCraft}
                    />
                  }
                />
              )}
              <p className="mt-3 rounded border border-edge-soft bg-panel-2 px-2.5 py-2 text-[10px] leading-relaxed text-ink-faint">
                You can reopen this wizard any time from Settings → About → Setup wizard.
              </p>
            </>
          )}
        </div>

        <div className="flex items-center gap-2 border-t border-edge-soft px-5 py-3">
          <button
            type="button"
            onClick={() => void skip()}
            disabled={busy}
            className="text-[11px] text-ink-faint underline underline-offset-2 transition-colors hover:text-ink disabled:opacity-40"
          >
            {firstRun ? "Skip setup" : "Cancel"}
          </button>

          <div className="ml-auto flex items-center gap-1.5">
            {step > 0 && (
              <button
                type="button"
                onClick={back}
                disabled={busy}
                className="flex items-center gap-1 rounded border border-edge bg-panel-2 px-2.5 py-1 text-[11px] text-ink-dim transition-colors hover:bg-panel-3 hover:text-ink disabled:opacity-40"
              >
                <IconChevronLeft className="size-3" />
                Back
              </button>
            )}
            {step < 2 ? (
              <button
                type="button"
                autoFocus
                onClick={() => void advance()}
                disabled={busy}
                className="rounded border border-accent-dim bg-accent/20 px-3 py-1 text-[11px] font-medium text-accent-hi transition-colors hover:bg-accent/30 disabled:opacity-40"
              >
                {busy ? "Saving…" : "Next"}
              </button>
            ) : (
              <button
                type="button"
                autoFocus
                onClick={() => void finish()}
                disabled={busy}
                className="rounded border border-accent-dim bg-accent/20 px-3 py-1 text-[11px] font-medium text-accent-hi transition-colors hover:bg-accent/30 disabled:opacity-40"
              >
                {busy ? "Saving…" : "Finish"}
              </button>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
