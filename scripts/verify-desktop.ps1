#requires -Version 7.0
[CmdletBinding()]
param([switch]$List, [switch]$Headless, [string]$PlaywrightModule, [string]$Chromium)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $false
Set-StrictMode -Version Latest
$repository = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $repository 'apps/desktop'
$plan = @(
    @{ name = 'frontend-tests'; tool = 'npm'; arguments = @('test'); directory = $desktop },
    @{ name = 'frontend-build'; tool = 'npm'; arguments = @('run', 'build'); directory = $desktop },
    @{ name = 'rust-tests'; tool = 'cargo'; arguments = @('test', '--offline'); directory = (Join-Path $desktop 'src-tauri') },
    @{ name = 'build-info-data-tests'; tool = 'pwsh'; arguments = @('-NoProfile', '-File', (Join-Path $PSScriptRoot 'test-desktop-build-info.ps1')); directory = $repository }
)
if ($Headless) {
    foreach ($name in @('workspace-files-interaction', 'conversation-input-performance', 'workbench-layout', 'conversation-delete', 'conversation-recovery')) {
        $plan += @{ name = $name; tool = 'node'; arguments = @("tests/$name-smoke.mjs"); directory = $desktop }
    }
}
if ($List) { $plan | ConvertTo-Json -Depth 4; return }
$originalLocation = Get-Location
$environmentNames = @('CARGO_NET_OFFLINE', 'AUDIOGRAPH_PLAYWRIGHT_MODULE', 'AUDIOGRAPH_CHROMIUM')
$savedEnvironment = @{}
foreach ($name in $environmentNames) { $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process') }
$logDirectory = Join-Path $repository ('build/desktop-verification/' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $logDirectory -Force | Out-Null
$steps = @($plan | ForEach-Object { [ordered]@{ name = $_.name; status = 'skipped'; exitcode = $null; duration_ms = 0; log = "$($_.name).log" } })
$summary = [ordered]@{ schema_version = 1; started_utc = [DateTime]::UtcNow.ToString('o'); status = 'failed'; git_head = $null; git_dirty = $null; steps = $steps; error = $null }
$failed = $false
try {
    # Fail before any tests if optional existing runtimes were not supplied.
    if ($Headless) {
        if (!$PlaywrightModule) { $PlaywrightModule = $env:AUDIOGRAPH_PLAYWRIGHT_MODULE }
        if (!$Chromium) { $Chromium = $env:AUDIOGRAPH_CHROMIUM }
        foreach ($dependency in @(@('PlaywrightModule', $PlaywrightModule), @('Chromium', $Chromium))) {
            if (!$dependency[1] -or !(Test-Path -LiteralPath $dependency[1] -PathType Leaf)) { throw "Headless requires an existing $($dependency[0]) file; nothing is downloaded." }
        }
        $env:AUDIOGRAPH_PLAYWRIGHT_MODULE = (Resolve-Path -LiteralPath $PlaywrightModule).Path
        $env:AUDIOGRAPH_CHROMIUM = (Resolve-Path -LiteralPath $Chromium).Path
    }
    $tools = @{}
    foreach ($name in @('npm', 'cargo', 'pwsh') + $(if ($Headless) { @('node') } else { @() })) {
        $binary = if ($IsWindows) { if ($name -eq 'npm') { 'npm.cmd' } else { "$name.exe" } } else { $name }
        # Multiple installations can be returned (for example system Git and
        # a bundled toolchain). Invoke the first PATH match, never a path array.
        $found = Get-Command $binary -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (!$found) { throw "Required command is missing: $binary" }
        $tools[$name] = $found.Source
    }
    if ($IsWindows -and (!(Get-Command cl.exe -CommandType Application -ErrorAction SilentlyContinue) -or !(Get-Command link.exe -CommandType Application -ErrorAction SilentlyContinue))) {
        throw 'VS C++ environment is missing: open Developer PowerShell for VS 2022 (x64), then rerun this script with PowerShell 7. Both cl.exe and link.exe must be available.'
    }
    $git = Get-Command git -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($git) {
        $gitArguments = @('-C', $repository, 'rev-parse', 'HEAD')
        $head = & $git.Source @gitArguments 2>$null
        if ($LASTEXITCODE -eq 0 -and $head -match '^[0-9a-f]{40,64}$') { $summary.git_head = "$head" }
        $status = & $git.Source -C $repository status --porcelain 2>$null
        if ($LASTEXITCODE -eq 0) { $summary.git_dirty = @($status).Count -gt 0 }
    }
    $env:CARGO_NET_OFFLINE = 'true'
    for ($index = 0; $index -lt $plan.Count; $index++) {
        $step = $plan[$index]; $record = $steps[$index]
        Set-Location -LiteralPath $step.directory
        $timer = [Diagnostics.Stopwatch]::StartNew()
        $record.status = 'running'
        try {
            $command = $tools[$step.tool]; $arguments = [string[]]$step.arguments
            Write-Host "Running $($step.name)"
            & $command @arguments 2>&1 | Tee-Object -FilePath (Join-Path $logDirectory $record.log)
            $record.exitcode = $LASTEXITCODE
            if ($record.exitcode -ne 0) { $record.status = 'failed'; throw "Step $($step.name) failed with exit code $($record.exitcode)." }
            $record.status = 'passed'
        } catch {
            $record.status = 'failed'
            if ($null -eq $record.exitcode) { $record.exitcode = 1 }
            throw
        } finally { $timer.Stop(); $record.duration_ms = [long]$timer.Elapsed.TotalMilliseconds }
    }
    $summary.status = 'passed'
} catch {
    $failed = $true
    $summary.error = $_.Exception.Message
    [IO.File]::WriteAllText((Join-Path $logDirectory 'failure.log'), $summary.error)
    Write-Warning $summary.error
} finally {
    Set-Location -LiteralPath $originalLocation.Path
    foreach ($name in $environmentNames) { [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process') }
    $summary['finished_utc'] = [DateTime]::UtcNow.ToString('o')
    $summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath (Join-Path $logDirectory 'summary.json') -Encoding utf8NoBOM
    Write-Host "Verification logs: $logDirectory"
}
if ($failed) { exit 1 }
