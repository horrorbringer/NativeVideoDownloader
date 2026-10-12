#!/usr/bin/env bash
set -e

# ==============================================================================
# Native Video Downloader - Docker Container Entrypoint
# ==============================================================================

RESOLUTION="${RESOLUTION:-1280x800x24}"
DISPLAY_NUM="${DISPLAY_NUM:-0}"

# If VNC / virtual display mode is enabled
if [ "${ENABLE_VNC:-false}" = "true" ]; then
    echo "=========================================================="
    echo " Starting Virtual Display & Web VNC (noVNC)"
    echo " Resolution: ${RESOLUTION}"
    echo " Web Access: http://localhost:6080/vnc.html"
    echo "=========================================================="

    export DISPLAY=":${DISPLAY_NUM}"
    export LIBGL_ALWAYS_SOFTWARE=1

    # Clean up any stale X11 lock files
    rm -f /tmp/.X${DISPLAY_NUM}-lock /tmp/.X11-unix/X${DISPLAY_NUM} 2>/dev/null || true

    # 1. Start Xvfb virtual framebuffer
    Xvfb :${DISPLAY_NUM} -screen 0 ${RESOLUTION} -nolisten tcp &
    XVFB_PID=$!
    sleep 1

    # 2. Start Fluxbox lightweight window manager
    fluxbox &
    FLUXBOX_PID=$!
    sleep 1

    # 3. Start x11vnc server
    x11vnc -display :${DISPLAY_NUM} -nopw -forever -shared -rfbport 5900 -quiet &
    X11VNC_PID=$!
    sleep 1

    # 4. Start noVNC via websockify (exposing port 6080)
    if [ -d "/usr/share/novnc" ]; then
        websockify --web=/usr/share/novnc 6080 localhost:5900 >/dev/null 2>&1 &
        WEBSOCKIFY_PID=$!
    fi

    echo ">>> Desktop environment is ready!"
    echo ">>> Open http://localhost:6080/vnc.html in your browser."
    echo "=========================================================="
fi

# If arguments are passed, execute them
if [ $# -gt 0 ]; then
    exec "$@"
else
    # Default to an interactive bash shell
    exec /bin/bash
fi
