# macOS port — handoff

Working notes for whoever picks this up. The phase plan is
[macos-support-plan.md](./macos-support-plan.md); this file records where the work
actually stands, how to run it, and what is left.

Branch: `agents/i-want-to-develop-the-mac-version-of` (PR #1).
Last updated 2026-11-23.

## Where it runs

| | |
|---|---|
| Test Mac | MacinCloud `NY702.macincloud.com`, user `user956782`, macOS 26.2, arm64, 8 cores / 16 GB |
| SSH | `ssh -i ~/.ssh/kept_macincloud_ed25519 user956782@NY702.macincloud.com` (key-only; the Mac password is not needed) |
| Repo on the Mac | `~/kept`, a shallow clone of the branch |
| Toolchain there | Node 22 in `~/.local/node`, cargo in `~/.cargo`, full Xcode, no Homebrew |
| Rebuild loop | `pwsh ./scripts/mac-dev.ps1` from the Windows checkout: copies changed files, builds the debug bundle, reopens it. `-Once` for a single pass |
| CI | `.github/workflows/ci.yml` on `macos-latest`: tests, bundles, launches the app, screenshots it, uploads `Kept-macos-gui` |

The Mac is a real desktop (1024x1280 over RDP), so the app window really does
appear. `screencapture` over SSH only works once `sshd-keygen-wrapper` is allowed
in System Settings → Privacy & Security → Screen & Audio Recording; without that
the CI artifact is the only view.

## Done

| Area | State |
|---|---|
| Build, bundle, launch | Works; ad-hoc signed, silently installs, tray icon appears |
| X11 gating | `x11rb` and its call sites are `target_os = "linux"`; the macOS graph has no x11rb |
| Clipboard capture | **Works** — text, HTML+RTF, images, file lists, `changeCount` as the sequence number, `org.nspasteboard.Concealed*` honoured |
| Clipboard write | **Works** — text with rich flavours, images, file URLs, file+text, clear |
| Frontmost app | Bundle identifier via `NSWorkspace`, so the ignore list matches on macOS |
| Sounds | `afplay` on the embedded WAVs, unpacked once into the temp folder |
| Shortcut labels | `src/lib/platform.ts` drives the hints; Print Screen rows are hidden on a Mac |
| Hotkey defaults | `Super+Shift+V` summon, `Super+Alt+P` recording pause, `Super+Alt+V` paste stack |
| Wizard chrome | Frameless on macOS with a drag handle in its header, 700 px tall |
| Screen capture | **Region, window and full screen** go through `screencapture` (`-i`, `-w -o`, `-x`). The Screen Recording grant is checked, asked for once and explained otherwise; the wizard has an "Allow…" button for it. "Previous area" repeats the rectangle, which is worked out by locating the selection inside a picture of the desktop taken just before it |
| Colour picker | **Works** — `NSColorSampler` puts up the system magnifier, which needs no Screen Recording grant; the answer is converted to sRGB before it is stored |
| OCR | **Works** — Vision's `VNRecognizeTextRequest` at accurate level with language correction. Offline, no permission, no model download; the fixture test reads "copypaste" and "2026" on the Mac |

Code lives in `src-tauri/src/macos_clipboard.rs` plus the `#[cfg(target_os = "macos")]`
sections of `clipboard.rs` and `sound.rs`. Everything else still falls through to
the **Linux** implementation, which is why the log shows `maim`, `xdotool` and
`tesseract` errors.

## Not done — in the order worth doing

1. **Auto-paste** (`paste.rs`). `xdotool` today. Needs `CGEventPost` of ⌘V plus
   reactivating the previous `NSRunningApplication`, gated on the Accessibility
   TCC grant.
2. **Screen recording** (`recorder.rs`). ffmpeg is not installed on the Mac; the
   existing "install ffmpeg" flow plus `-f avfoundation` input is the port.
   Region and window recording are still refused on macOS (`choose_region`,
   `choose_window`): ffmpeg records a screen, not a rectangle, until a selector
   reports one.
4. **Scrolling capture** (`capture.rs`) is refused on macOS for the same reason:
   it needs a window rectangle and a way to wheel it.
5. **The `::` popup** (`autocomplete.rs`). Deliberately inert on macOS: there is
   no non-grabbing key recorder yet. It records the token and installs no hook.
6. **Dock / app icon.** The `.icns` is older artwork than the current logo;
   regenerate with `npm run tauri icon <source.png>`.
7. **"Show in taskbar"** should map to `NSApplicationActivationPolicy::Accessory`
   on macOS (`window::set_taskbar_visibility` is a no-op there today).

## How to verify a change

```powershell
# from the Windows checkout
pwsh ./scripts/mac-dev.ps1 -Once          # sync + debug build + relaunch
ssh -i "$HOME/.ssh/kept_macincloud_ed25519" user956782@NY702.macincloud.com `
  'tail -n 20 ~/Library/Logs/ikept.it/Kept.log'
```

Clipboard checks that need no GUI:

```sh
DB="$HOME/Library/Application Support/ikept.it/copypaste.db"
printf 'some text' | pbcopy; sleep 1
sqlite3 "$DB" "select id, kind, char_count, html is not null, rtf is not null from clips order by id desc limit 3;"
```

Rich text needs a real pasteboard write; a two-line Swift script against AppKit
(`NSPasteboard.general.setString(_:forType:.html)`) works, as does copying from a
browser in the RDP session.

## Traps found the hard way

- `mac-dev.ps1` only copied the paths it watches; `Cargo.toml` was not one of
  them, so a new macOS dependency built here and failed there with an unresolved
  import. It is watched now.
- `open` over SSH fails with `RBSRequestErrorDomain Code=5` while the console
  belongs to another user (the MacinCloud box has three). `who -u` says whose
  session is on the console; retry when it is `user956782` again.
- `ssh-keygen -N '""'` in PowerShell stores the literal characters `""` as the
  passphrase, and the client then never signs with the key even though the server
  says `Server accepts key`.
- `tauri build` needs `--config '{"bundle":{"createUpdaterArtifacts":false}}'`
  off the release workflow, or it asks for the updater signing key.
- The wizard at 560x780 does not fit a 768-tall desktop; centring hides its head.
- `cargo test` is the gate that catches platform defaults: three tests asserted
  the literal `Alt+Shift+V` when the stack chord became platform-specific.
