#!/usr/bin/env bash
set -e

# ==============================================================================
# Convenient Docker development helper for Native Video Downloader
# ==============================================================================

COMMAND="${1:-gui}"

case "$COMMAND" in
    gui)
        echo -e "\033[36m>>> Starting Native Video Downloader with Web noVNC...\033[0m"
        echo -e "\033[32m>>> Open: http://localhost:6080/vnc.html\033[0m"
        docker compose up app-gui
        ;;
    watch)
        echo -e "\033[36m>>> Starting Native Video Downloader with Live Auto-Reload...\033[0m"
        echo -e "\033[32m>>> Open: http://localhost:6080/vnc.html\033[0m"
        docker compose up app-gui-watch
        ;;
    cli)
        echo -e "\033[36m>>> Opening interactive bash shell inside dev container...\033[0m"
        docker compose run --rm app-cli
        ;;
    test)
        echo -e "\033[36m>>> Running test suite...\033[0m"
        docker compose run --rm app-test
        ;;
    build)
        echo -e "\033[36m>>> Building release binary...\033[0m"
        docker compose run --rm app-build
        ;;
    down)
        echo -e "\033[33m>>> Stopping all containers...\033[0m"
        docker compose down
        ;;
    *)
        echo "Usage: ./docker-dev.sh [gui|watch|cli|test|build|down]"
        exit 1
        ;;
esac
