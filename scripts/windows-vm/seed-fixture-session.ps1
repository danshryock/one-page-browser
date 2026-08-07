# Pre-seeds browser-windows-reactor's "default" profile session with every
# web-standards-tests fixture case already open as its own page, before the
# app is ever launched — see web-standards-tests/src/bin/windows_driver.rs's
# `drive_and_capture` doc comment for why: switching to an *already-open*
# page (`switch_to`) is the reliable code path; creating a brand new one via
# the switcher's search box (`do_add_page`) has a real, separate bug where
# the new page's visibility doesn't reliably take effect on the render it
# first becomes active in, leaving whichever page was previously showing
# still the one that's actually visible and receiving clicks.
#
# Usage: powershell -ExecutionPolicy Bypass -File seed-fixture-session.ps1
$dir = "C:\Users\Docker\AppData\Roaming\claude-browser\config\default"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
$json = '{"pages":[{"url":"https://www.google.com","title":"Home"},{"url":"file:///C:/ClaudeBrowser/fixtures/opener-default/index.html","title":"opener-default"},{"url":"file:///C:/ClaudeBrowser/fixtures/opener-explicit-opener/index.html","title":"opener-explicit-opener"}],"active_index":0}'
# Explicitly no-BOM UTF-8 — `Set-Content -Encoding utf8` prepends a BOM on
# Windows PowerShell 5.1, which breaks `serde_json::from_str` silently (no
# error, `Session::load` just falls back to an empty session) — confirmed
# directly by reading the file back and seeing the raw BOM bytes.
$utf8NoBom = New-Object System.Text.UTF8Encoding $false
[System.IO.File]::WriteAllText((Join-Path $dir "session.json"), $json, $utf8NoBom)
Write-Output "session.json written"
