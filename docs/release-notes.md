<!-- Notes for the next release. The Release workflow publishes this file as the
     GitHub release body and as the `notes` field of latest.json, which is what
     Kept's update window shows, so rewrite it before tagging. -->

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
