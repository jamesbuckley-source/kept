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

- `cargo check --all-targets` is green, and `npm run tauri dev` builds and opens
  the app on the container desktop (setup wizard, SQLite migrations, global
  shortcuts and the tray all come up).
- Clipboard **writes** use `arboard` on X11 (`write_flavoured`); rich HTML/RTF
  flavours and X11 file lists are not carried yet.
- Capture, OCR, paste injection and the printscreen hooks are still the Win32
  implementations only; the Linux side of those traits has not been written.

## Known limits

- No GPU: WebKit runs on llvmpipe, so animations are slow and 3D effects are off.
- The clipboard is the container's own X clipboard. Kept inside the container
  cannot see Windows' clipboard.
- macOS cannot be containerised. macOS-specific code is validated in CI
  (`macos-14` runner) or on a real Mac, never here.
- OCR is tesseract, not `Windows.Media.Ocr`, so results differ from Windows.
