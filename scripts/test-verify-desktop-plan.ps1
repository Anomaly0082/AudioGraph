#requires -Version 7.0
# Only plan serialization and fail-fast input validation; never invokes real tests.
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest
$runner = Join-Path $PSScriptRoot 'verify-desktop.ps1'
$logRoot = Join-Path (Split-Path -Parent $PSScriptRoot) 'build/desktop-verification'
$before = @(Get-ChildItem -LiteralPath $logRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object Name)
$normal = & pwsh -NoProfile -File $runner -List | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $normal.Count -ne 4) { throw 'Default plan must have exactly four steps.' }
$browser = & pwsh -NoProfile -File $runner -List -Headless | ConvertFrom-Json
if ($LASTEXITCODE -ne 0 -or $browser.Count -ne 9) { throw 'Headless plan must have exactly nine steps.' }
$after = @(Get-ChildItem -LiteralPath $logRoot -Directory -ErrorAction SilentlyContinue | ForEach-Object Name)
if (@(Compare-Object $before $after).Count -ne 0) { throw '-List created output directories.' }
if (($normal[2].arguments -join ' ') -cne 'test --offline') { throw 'Rust plan must only run ordinary offline tests.' }
if (($browser[4].arguments -join ' ') -cne 'tests/workspace-files-interaction-smoke.mjs' -or ($browser[5].arguments -join ' ') -cne 'tests/conversation-input-performance-smoke.mjs' -or ($browser[6].arguments -join ' ') -cne 'tests/workbench-layout-smoke.mjs') { throw 'Unexpected browser test plan.' }
if (($browser[7].arguments -join ' ') -cne 'tests/conversation-delete-smoke.mjs') { throw 'Conversation deletion regression must remain in the headless plan.' }
if (($browser[8].arguments -join ' ') -cne 'tests/conversation-recovery-smoke.mjs') { throw 'Conversation recovery regression must remain in the headless plan.' }
if (($browser | ConvertTo-Json -Depth 4) -match '--ignored|tauri.*build|native_webview|recycle') { throw 'Unexpected native or application launch route.' }
$missing = Join-Path ([IO.Path]::GetTempPath()) ([guid]::NewGuid().ToString('N') + '.missing')
$diagnostic = & pwsh -NoProfile -File $runner -Headless -PlaywrightModule $missing -Chromium $missing 2>&1
if ($LASTEXITCODE -eq 0 -or ($diagnostic -join "`n") -notmatch 'Headless requires an existing PlaywrightModule file') { throw 'Missing headless dependencies must fail before testing.' }
$newLogs = @(Get-ChildItem -LiteralPath $logRoot -Directory | Where-Object Name -NotIn $before)
if ($newLogs.Count -ne 1) { throw 'Preflight failure must retain one unique log directory.' }
$summary = Get-Content -LiteralPath (Join-Path $newLogs[0].FullName 'summary.json') -Raw | ConvertFrom-Json
if ($summary.status -ne 'failed' -or @($summary.steps | Where-Object status -NE 'skipped').Count -ne 0) { throw 'Preflight failure ran actual steps.' }
Write-Host 'Desktop verification plan: 7 checks passed; no frontend, Cargo, browser or native tests were launched.'
