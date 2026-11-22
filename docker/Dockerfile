# syntax=docker/dockerfile:1.7
#
# Kept's Linux development container: a 1920x1080 X desktop with noVNC on port
# 6080, plus everything the Tauri side needs to build and run.
#
#   docker compose -f docker/compose.yaml build
#   docker compose -f docker/compose.yaml up -d
#   open http://localhost:6080/vnc.html

FROM debian:trixie-slim AS runtime

ENV DEBIAN_FRONTEND=noninteractive \
    DISPLAY=:99 \
    LIBGL_ALWAYS_SOFTWARE=1 \
    WEBKIT_DISABLE_COMPOSITING_MODE=1 \
    WEBKIT_DISABLE_DMABUF_RENDERER=1 \
    GDK_BACKEND=x11

# Desktop, the browser that shows it, and the Linux stand-ins for the Win32
# APIs Kept uses on Windows (capture, OCR, paste, recording).
#
# The GStreamer extras are what WebKit decodes video with: `libav` carries the
# H.264 decoder every recording is encoded with, and `plugins-bad` the parser in
# front of it. Without them a recording is a black rectangle with no explanation.
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb openbox stalonetray x11vnc novnc websockify \
      chromium ffmpeg maim xdotool xclip \
      tesseract-ocr tesseract-ocr-eng \
      dbus-x11 xdg-utils x11-utils x11-xserver-utils \
      fonts-dejavu-core fonts-liberation fonts-noto-color-emoji \
      libgl1-mesa-dri libegl1 mesa-utils \
      procps iproute2 netcat-openbsd sudo less \
      ca-certificates curl wget git unzip xz-utils file jq \
      libwebkit2gtk-4.1-0 libgtk-3-0 libayatana-appindicator3-1 \
      librsvg2-2 libxdo3 libasound2t64 \
      gstreamer1.0-libav gstreamer1.0-plugins-bad \
    && rm -rf /var/lib/apt/lists/*

COPY docker/entrypoint.sh /usr/local/bin/kept-desktop
# Checked out on Windows the script can arrive with CRLF, which makes the
# kernel look for an interpreter called "/bin/sh\r".
RUN sed -i 's/\r$//' /usr/local/bin/kept-desktop && chmod +x /usr/local/bin/kept-desktop

EXPOSE 6080 5173 1420
ENTRYPOINT ["/usr/local/bin/kept-desktop"]


FROM runtime AS dev

# Tauri's Linux build dependencies.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config libssl-dev \
      libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
      libjavascriptcoregtk-4.1-dev \
      libayatana-appindicator3-dev librsvg2-dev libxdo-dev \
      clang lld mold \
      patchelf libfuse2 rpm \
    && rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH \
    RUST_BACKTRACE=1

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain stable \
    && rustup component add clippy rustfmt \
    && rustc --version && cargo --version

# Node 22 for Vite 8 and the Tauri CLI.
RUN set -eux; \
    arch="$(uname -m)"; \
    case "$arch" in x86_64) narch=x64 ;; aarch64) narch=arm64 ;; *) echo "unsupported arch $arch" >&2; exit 1 ;; esac; \
    file="$(curl -fsSL https://nodejs.org/dist/latest-v22.x/ | grep -o "node-v[0-9.]*-linux-${narch}\.tar\.xz" | head -1)"; \
    curl -fsSLO "https://nodejs.org/dist/latest-v22.x/${file}"; \
    tar -xJf "${file}" -C /usr/local --strip-components=1 --no-same-owner; \
    rm -f "${file}"; \
    node --version && npm --version

WORKDIR /workspace
CMD ["sleep", "infinity"]
