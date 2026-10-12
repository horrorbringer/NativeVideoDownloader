<#
.SYNOPSIS
    Convenient Docker development helper for Native Video Downloader.
.DESCRIPTION
    Usage:
        .\docker-dev.ps1 gui       # Run GUI app accessible in browser at http://localhost:6080
        .\docker-dev.ps1 watch     # Run GUI with live auto-reload (cargo-watch)
        .\docker-dev.ps1 cli       # Open an interactive bash terminal inside container
        .\docker-dev.ps1 test      # Run all cargo tests
        .\docker-dev.ps1 build     # Build production Linux release binary
        .\docker-dev.ps1 down      # Stop and remove containers
#>

param (
    [ValidateSet("gui", "watch", "cli", "test", "build", "down", "help")]
    [string]$Command = "gui"
)

switch ($Command) {
    "gui" {
        Write-Host ">>> Starting Native Video Downloader with Web noVNC..." -ForegroundColor Cyan
        Write-Host ">>> Once started, open: http://localhost:6080/vnc.html" -ForegroundColor Green
        docker compose up app-gui
    }
    "watch" {
        Write-Host ">>> Starting Native Video Downloader with Live Auto-Reload..." -ForegroundColor Cyan
        Write-Host ">>> Once started, open: http://localhost:6080/vnc.html" -ForegroundColor Green
        docker compose up app-gui-watch
    }
    "cli" {
        Write-Host ">>> Opening interactive bash shell inside dev container..." -ForegroundColor Cyan
        docker compose run --rm app-cli
    }
    "test" {
        Write-Host ">>> Running test suite..." -ForegroundColor Cyan
        docker compose run --rm app-test
    }
    "build" {
        Write-Host ">>> Building release binary..." -ForegroundColor Cyan
        docker compose run --rm app-build
    }
    "down" {
        Write-Host ">>> Stopping all containers..." -ForegroundColor Yellow
        docker compose down
    }
    "help" {
        Get-Help $MyInvocation.MyCommand.Path
    }
}
