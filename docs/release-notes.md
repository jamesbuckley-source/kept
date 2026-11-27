<!-- Notes for the next release. The Release workflow publishes this file as the
     GitHub release body and as the `notes` field of latest.json, which is what
     Kept's update window shows, so rewrite it before tagging. -->

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

## Packaging and docs

- Release automation builds and uploads Linux packages for tagged releases.
- Documentation updated to describe Linux Beta status, Linux installation options, and
  current Linux limitations (including the ruler).
