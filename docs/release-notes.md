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

## QR codes

A picture in the history that contains a QR code is read for you: select it and the
preview pane adds a **QR code** row with what the code holds, plus an **Open link**
button when what it holds is an address. Nothing about it is stored, so a picture
captured before this release reads the same way as one captured after it, and a code
printed without a scheme (`example.com/order/42`) is opened as `https`.

There is a new capture tool beside the others: **Capture QR code**, in the capture
menu, the sidebar strip and the tray's Capture submenu, frames a region exactly as a
screenshot does and keeps the code's payload as a text clip, which puts it on the
clipboard. A code that holds a link opens in your browser as well, so scanning a
poster or a screen is one drag rather than a drag, a paste and a click. An area with
no code in it says so and adds nothing.

- **The same decoder on every platform.** Reading codes is pure Rust, so Windows,
  macOS and Linux decode the same way, with nothing extra to install.
- **Fast on ordinary screenshots.** Large pictures are scaled down before they are
  searched, and the search only runs when a picture is on screen, never in the
  background.

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

## Add Clip Note

The *Add clip* button is now *Add Clip Note*, and what it writes is kept: every note
lands in **Utilities -> Clip notes**, an archive of its own with a count beside it,
the usual card and list layouts, and search that stays inside the notes. Because it
is an archive, retention and every bulk delete leave notes alone, and the header
says so: *Clip notes - kept forever*. `Ctrl`+`N` opens the same window from any view.

A note whose text is already in your history is left where it is rather than being
pulled into the archive, so a clip you captured keeps belonging to where it came
from.

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

## KeptCraft, installed from Kept

*Settings -> Image editor* and the setup wizard each carry **Install KeptCraft**.
Pressing it finds the newest release on GitHub, picks the build for this machine, and
downloads it over HTTPS, checking the file against the SHA-256 the release itself
publishes before anything runs - a build without a published digest is never
offered. Settings names the version, file and size first, so the download is a second
click and not a surprise.

What happens next is the platform's own installer: an MSI wizard on Windows, the disk
image on macOS, the AppImage or the desktop's package handler on Linux. Once it has
finished, *Detect KeptCraft* fills the editor command in as before.

## What's new, where the update is offered

The band that announces an available update used to say only that one existed. It now
lists what is in the release, headline by headline, under the version, with *+N more*
when the list runs on, and the existing *What's new* link beside it for the full
notes. *Settings -> About -> Show latest updates* always has the real notes to show:
the pending release's when one is waiting, and otherwise the notes that ship inside
the build you are running, so the panel reads the same offline as it does online.

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

The Linux packages for 1.3.0 were never attached: two of the new KeptCraft tests
asserted against the platform they were compiled for, so they passed on Windows and
failed on the Linux runner. They now name the platform they are asking about. 1.3.1
is the same release with that fixed, which is why the `.deb`, `.AppImage` and `.rpm`
files are here.

## Packaging and docs

- Release automation builds and uploads Linux packages for tagged releases.
- Documentation updated to describe Linux Beta status, Linux installation options, and
  current Linux limitations (including the ruler).
