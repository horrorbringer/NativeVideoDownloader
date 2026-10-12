# ==============================================================================
# Native Video Downloader - Multi-Option Dockerfile
# Supports:
#   1. Web-based GUI development (noVNC in browser at http://localhost:6080)
#   2. Fast CLI development (cargo check / cargo test / cargo watch)
#   3. VS Code Remote Dev Containers (.devcontainer)
#   4. Production Linux binary build & export
# ==============================================================================

FROM rust:1.85-bookworm AS dev

ENV DEBIAN_FRONTEND=noninteractive

# 1. Install build tools, Slint GUI dependencies, FFmpeg, SQLite, & Virtual Display tools
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    pkg-config \
    libssl-dev \
    curl \
    git \
    ca-certificates \
    # Slint GUI Linux dependencies
    libfontconfig1-dev \
    libxcb1-dev \
    libx11-xcb-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-dev \
    libwayland-dev \
    # Software OpenGL rasterizer (enables rendering without dedicated GPU passthrough)
    libgl1-mesa-dri \
    libgl1-mesa-glx \
    libegl1-mesa \
    libfontconfig1 \
    # Media tools & runtime dependencies
    ffmpeg \
    sqlite3 \
    libsqlite3-dev \
    # Virtual display & web streaming (Xvfb + Fluxbox + x11vnc + noVNC)
    xvfb \
    fluxbox \
    x11vnc \
    novnc \
    websockify \
    python3 \
    && rm -rf /var/lib/apt/lists/*

# 2. Install latest yt-dlp binary
RUN curl -L https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp -o /usr/local/bin/yt-dlp \
    && chmod a+rx /usr/local/bin/yt-dlp

# 3. Install cargo-binstall for fast pre-compiled cargo tooling (cargo-watch)
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash \
    && cargo binstall -y --no-confirm cargo-watch \
    && rm -rf /root/.cargo/git /root/.cargo/registry/cache

# 4. Configure app directory and environment defaults
WORKDIR /app

ENV SLINT_BACKEND=winit
ENV LIBGL_ALWAYS_SOFTWARE=1
ENV RUST_LOG=info
ENV RUST_BACKTRACE=1

# 5. Set up entrypoint
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# Expose noVNC web port (6080) and raw VNC port (5900)
EXPOSE 6080 5900

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
CMD ["/bin/bash"]
