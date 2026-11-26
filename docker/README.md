# Developing Kept on Linux in Docker

A Debian container with a full 1920×1080 X desktop you can open in a browser tab.
It exists to build, run, test and record Kept on Linux, and to be the reference
environment for the cross-platform work.

## What is in it

| Piece | Why |
| --- | --- |
| Xvfb `:99` + openbox + stalonetray | the desktop, and a tray for Kept's icon |
| x11vnc + noVNC | the desktop, in a browser at `http://localhost:6080/vnc.html` |
| Chromium | to open `docs/demo.html` inside the same desktop |
| ffmpeg, maim, xdotool, xclip | the Linux stand-ins for the Win32 capture / paste code |
| tesseract | the Linux stand-in for `Windows.Media.Ocr` |
| Rust stable + clippy + rustfmt | `src-tauri` |
| Node 22 + npm | Vite and the Tauri CLI |

Image is `kept-dev:latest`, built from the `dev` stage (~2.6 GB).

## Run it

```powershell
docker compose -f docker/compose.yaml build   # first time, and after Dockerfile changes
docker compose -f docker/compose.yaml up -d
```

Then open **http://localhost:6080/vnc.html** (no password) for the desktop.

```powershell
docker exec -it kept-dev bash                 # a shell in /workspace
docker compose -f docker/compose.yaml down    # stop
```

VS Code: *Dev Containers: Open Folder in Container* uses
[`.devcontainer/devcontainer.json`](../.devcontainer/devcontainer.json), which
runs `npm ci` on creation.

## Day-to-day

```bash
npm ci                     # dependencies (first run only)
npm run dev                # Vite alone, on :5173
npm run tauri dev          # the app, inside the desktop
npm run build              # tsc + vite build
cd src-tauri && cargo test # Rust tests
```

Anything GUI opens on the Xvfb desktop, so run it, then look at the noVNC tab.
Chromium needs `--no-sandbox` because the container runs as root:

```bash
chromium --no-sandbox --new-window http://127.0.0.1:5299/demo.html
```

## Recording demos

`x11grab` records the whole desktop **with the cursor burnt in**, which is what
the OpenScreen takes had to fake:

```bash
ffmpeg -f x11grab -draw_mouse 1 -video_size 1920x1080 -framerate 30 \
       -i :99 -c:v libx264 -crf 20 -pix_fmt yuv420p demo.mp4
```

Kept's own recorder shells out to ffmpeg on Windows already, so once the Linux
port lands the same `-f x11grab` path works from inside the app.

## Caches and volumes

`node_modules`, `src-tauri/target`, `dist` and the Chromium profile are Docker
volumes, not part of the Windows bind mount — mixing Linux binaries into a
Windows-built `node_modules`, or compiling `target/` across a WSL 2 bind mount,
is slow at best. Delete them to start clean:

```powershell
docker compose -f docker/compose.yaml down -v
```

## Where the Linux port stands

Linux support is **Beta**. `cargo check --all-targets` is green, `cargo test --lib`
is **458 passing, 0 failing** on Linux (475 on Windows), and the packaged app opens
on the container desktop — setup wizard, SQLite migrations, global shortcuts and the
tray all come up. The release workflow now builds Linux packages (AppImage, `.deb`,
`.rpm`) for end users alongside the Windows installer. Most functionality should work
on Linux (X11), including the onscreen ruler.

Implemented for X11:

| Area | Linux implementation |
| --- | --- |
| Clipboard writes | one `arboard` context lives for the whole process, because an X11 selection dies with the connection that set it; file lists go out as `text/uri-list` through `xclip`, and the watcher reads that target back |
| Clipboard auto-clear | the same owner clears by publishing an empty selection, which is the X11 equivalent of an empty clipboard |
| Animated GIFs and videos behind a copy | a browser's "Copy image" puts only the frame on screen on the clipboard, so the `text/html` target is read with `xclip` and the asset it names is fetched and kept beside the frame; copying that clip publishes the file as `text/uri-list`, so pasting keeps every frame |
| Full-screen capture | `maim --hidecursor` → RGBA (`capture_screen`, covered by `capture::linux_tests`) |
| Region capture | `maim --select`, whose own X11 overlay is the drag surface |
| Window capture | `xdotool selectwindow` picks the window, `maim --window <id>` grabs it |
| Scrolling capture | the same tested stitcher as Windows, driven by `maim -g` frames and `xdotool` wheel/Page Down; the pointer is parked outside the page between steps, and the window's own bottom edge is measured out of each frame so a join never repeats it as a light line |
| Colour picker | the screen is frozen, `xdotool`'s crosshair takes the click, and the pixel under it is read back |
| Screen recording | `ffmpeg -f x11grab` with the origin in the input name; audio stays a Windows choice |
| Playing a recording back | WebKitGTK's media player only opens what GStreamer has a source for, and a custom scheme is not one, so recordings are streamed from a loopback HTTP port with a per-run path secret; the handler, allowlist, sealing and ranges are the same ones the `media:` scheme uses, and images stay on the scheme |
| `::` quick paste | the RECORD extension reports every key press without grabbing the keyboard, so the token is noticed in whichever app has the focus and the keycodes are mapped to characters through the keyboard map |
| Paste, stack paste, auto-paste | `xdotool key ctrl+v` after `windowactivate --sync`, with the clipboard owner kept alive so the paste has something to read |
| Desktop bounds | `xdotool getdisplaygeometry` |
| Trigger backspaces | `xdotool key --repeat BackSpace` |
| OCR | `tesseract --psm 6` on a temporary PNG |
| Overlay windows | opaque when no compositor is running, which is what the container's bare X session is |
| Cloud folders | Dropbox roots from `~/.dropbox/info.json`, plus the `My Drive` / `Google Drive` folder names |
| Countdown / floating UI (no compositor) | the screenshot countdown window is clipped to its ring via X11 SHAPE on bare X sessions, and the screen recorder bar opens at a compact initial size so it does not show excess bottom padding while settling |

Still limited or not yet available on Linux (Beta):

- **Onscreen ruler** is a transparent full-screen webview on Linux (`ruler.rs`'s
  `linux` module) painted by `src/components/Ruler.tsx`; the Windows GDI overlay
  remains the reference for its behaviour. On a session without a compositor the
  overlay paints the pre-open screenshot as its own backdrop, and snapping to
  colour edges needs `maim` (X11). Window snapping sees XWayland windows only.
- the recorder's region and window placement (`choose_region` / `choose_window`),
  which share the Win32 drag surface
- Print Screen as a global key: the option registers a `PrintScreen` shortcut, and
  on this X server the grab is refused as already held. Captures stay reachable
  from any capture shortcut set in Settings → Capture
- `reveal_path` / `open_path`: they go through the XDG desktop portal, and a bare X
  session has no session bus (and the container has no file manager)

## Known limits

- No GPU: WebKit runs on llvmpipe, so animations are slow and 3D effects are off.
- Video needs `gstreamer1.0-libav` and `gstreamer1.0-plugins-bad` for the H.264 every
  recording is encoded with; the image installs both, and without them a recording
  plays as a black rectangle.
- No compositor: transparent windows cannot be blended, so the recording bar, the
  countdown and the record frame are drawn opaque/clipped instead of smoothly blended.
- The clipboard is the container's own X clipboard. Kept inside the container
  cannot see Windows' clipboard.
- A scrolled capture costs about a second per step on llvmpipe, so a long page
  takes a minute or two.
- macOS cannot be containerised. macOS-specific code is validated in CI
  (`macos-latest` runner) or on a real Mac, never here.
- OCR is tesseract, not `Windows.Media.Ocr`, so results differ from Windows.

