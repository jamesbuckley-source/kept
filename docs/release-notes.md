## Themes

The two-way light/dark switch is now a theme system, all of it in
*Settings -> Appearance -> Themes*.

- **Eight palettes in one row:** Dark and Light sit beside Midnight, Slate,
  Sandstone, Forest, Mono and High contrast, each with a swatch of what it paints.
  One press sets all twelve colours at once, so a theme never leaves a stray tone
  from the one before it behind.
- **The capture overlays follow the theme.** A palette can carry the colour the
  region border, window highlight and size chip draw with; it lands in the outline
  setting, so it can still be changed afterwards.
- **Customize** is one press away underneath, and opens the picker for every
  colour - the same tokens as before, just no longer a separate mode to find.
- **Import theme** takes a `.zip` holding a `theme.json` (or a bare `theme.json`)
  and adds it to the row. Every colour in the file must be present and a real hex
  value, and the importer reads that one file and nothing else, so a theme cannot
  bring anything else along with it. Imported themes are stored as one small file
  each under the app data folder, which means a theme survives moving your history
  folder and can be shared by copying a single file. A theme whose name matches a
  bundled one keeps its own id instead of shadowing it, and imported ones carry a
  small remove button.

## Stacks live in their own window

The paste queue - formerly "Paste stack" - is now one thing instead of three. The
stack badge in the title bar and *Utilities -> Stacks* both open it in a small
frameless window that stays above other applications, so it can sit beside the
editor being pasted into. The anchored popover, and the pop-out-and-dock dance
between the two, are gone.

- **Paste next** reads as an icon and its word, because it is the action the panel
  expects a press of; **Paste all** and **Copy all** stay compact icon buttons with
  tooltips.
- **Paste all** pastes the whole queue in order and keeps it: each item goes on the
  clipboard in its own native shape - a picture as a picture, text as text, files
  as files - so nothing depends on the target reconstructing one combined payload.
- **Drag files or folders** from the desktop onto that window and they are imported
  and queued at the end, with the status line reporting how many. A file already in
  the history queues the clip that is there rather than making a second copy.

## Pasting pictures that works everywhere

A copied picture used to be published as a bitmap only, which left every
application to make of it what it could. It now carries four flavours in one
clipboard: the bitmap Office and Paint read, the registered `PNG` format that
Chromium-based editors such as Google Docs look for, an HTML image for canvas
editors, and **a real file** - so copying a screenshot in Kept and pasting it into
a folder writes `capture-<timestamp>.png` instead of doing nothing.

**Copy all** was hardened in the same pass:

- Its RTF is written with signed `\u` escapes and validated before it is published.
  An emoji anywhere in the queue used to make the RTF unreadable, and Word answers
  an unreadable flavour by dropping it and pasting the plain text - the images'
  file paths - which is what made "Copy all" look broken in Word.
- Picture sizes are scaled into the 16-bit range an RTF control word allows, and
  images too large to embed are referenced by `file://` URL in the HTML.
- A `.svg` dropped into Kept stays a file instead of becoming a text clip, so
  pasting it pastes the drawing rather than its markup.

## Pasting goes back where you were

`Alt`+`Shift`+`V` and **Paste next** paste into the window you were working in
before Kept came forward. Kept now remembers the last window that was in front
even when the queue is opened from its own interface, so the button no longer
depends on having summoned Kept with the shortcut first - and a queue item is only
used up when the paste actually landed.

## Faster clip entry

*New clip* saves as you type: the clip is created, or updated, a moment after you
stop typing, with **Copy as new** and **Done** beside it, so a note never depends
on remembering to press save.

## Capture text

A new capture kind takes the same region drag a screenshot does, but keeps the
words instead of the picture: frame a log line, an error dialog or a passage in a
PDF and it lands in history as a text clip, already on the clipboard, ready to
paste into an editor. Recognition is the same on-device engine that already reads
text out of screenshots — Windows' OCR, Vision on macOS, `tesseract` on Linux —
and an area with no words in it adds nothing and says so.

**Capture text** sits in the capture menu, the tray menu and *Settings → Capture
→ Capture shortcuts*, where it can be given a shortcut of its own. It behaves as
the other kinds do: the app hides for the capture, the delay applies, and Print
Screen runs it when it is the remembered capture type.

## Linux support (Beta)

Kept can now be built and packaged for Linux (X11) as a **Beta**. Tagged releases
include Linux install artifacts (`.AppImage`, `.deb`, `.rpm`) alongside the Windows
installer so end users can install Kept on Linux from the GitHub Releases page.

Most functionality should work on Linux, including the onscreen ruler; capture,
clipboard, tray, and recording features are available subject to the usual
X11/desktop-environment limitations documented in `docker/README.md`.

## Onscreen ruler on Linux

The ruler now opens on Linux as a transparent full-screen overlay that draws the
same tools, snapping, readouts and keys as the Windows version.

- **X11** behaves as described for Windows, including Esc to close.
- **Wayland** also opens, but the compositor hands it no global keys, so a locked
  ruler is released with the **Unlock** button it shows. Colour-edge snapping needs
  a screenshot, which needs X11 (`maim`); window snapping sees XWayland windows only.
- Sessions without a compositor paint the screenshot taken just before the ruler
  opened as the overlay's backdrop, so it does not cover the screen with an opaque
  rectangle.

## Linux fixes

- **Screenshot countdown edges:** on non-composited X11 sessions, the countdown window
  is clipped to a circular shape (X11 SHAPE) more reliably so transparent corners do
  not appear as square/opaque artifacts.
- **Screen recorder bar sizing:** the floating recorder bar now opens at a compact
  initial height on Linux to avoid excess empty padding at the bottom while its
  content-driven resize settles.
- **Tray menu:** added **“Open Kept”** as the top tray menu item to quickly show/focus
  the main window.

## Linux packages

The Linux build for 1.2.0 stopped on a compile error in the picture-clipboard
code, which is Windows-only; 1.2.1 is the same release with that fixed, so the
`.deb`, `.AppImage` and `.rpm` packages are attached here.

## Packaging and docs

- Release automation builds and uploads Linux packages for tagged releases.
- Documentation updated to describe Linux Beta status, Linux installation options, and
  current Linux limitations (including the ruler).
