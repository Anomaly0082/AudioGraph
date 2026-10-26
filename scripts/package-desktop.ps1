#requires -Version 7.0
[CmdletBinding()]
param(
    [switch]$LocalDevelopmentOnly,
    [string]$Executable,
    [string]$Destination
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path -Parent $PSScriptRoot
$runtime = Join-Path $repository 'apps/desktop/src-tauri/resources/ffmpeg'
$provenancePath = Join-Path $runtime 'bundle-provenance.json'
if (!(Test-Path -LiteralPath $provenancePath -PathType Leaf)) { throw 'FFmpeg resources missing; run scripts/prepare-ffmpeg.ps1 first.' }
$provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json
. (Join-Path $PSScriptRoot 'ffmpeg-audio-distribution.ps1')
$provenance = Assert-FfmpegAudioDistribution -Root $runtime
$expectedProfile = if ($LocalDevelopmentOnly) { 'debug' } else { 'release' }
$applicationName = if ($LocalDevelopmentOnly) { 'AudioProcess-local-development.exe' } else { 'AudioProcess.exe' }
if (!$Executable) { $Executable = Join-Path $repository "apps/desktop/src-tauri/target/$expectedProfile/audioprocess-prototype.exe" }
if (!$Destination) { $Destination = Join-Path $repository $(if ($LocalDevelopmentOnly) { 'build/AudioProcess-local-development' } else { 'build/AudioProcess-portable' }) }
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
$destinationPath = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $destinationPath) { throw 'Local development destination already exists; choose a new directory. Nothing was overwritten.' }
$sidecar = Join-Path (Split-Path -Parent $executablePath) 'control-cli.exe'
if (!(Test-Path -LiteralPath $sidecar -PathType Leaf)) { throw 'Required control-cli.exe sidecar is missing; no local copy was created.' }
. (Join-Path $PSScriptRoot 'desktop-build-info.ps1')
$buildMode = Read-DesktopEmbeddedBuildInfo -Executable $executablePath -ExpectedProfile $expectedProfile
foreach ($file in $provenance.files) {
    if ($file.name -match '[/\\:]|^\.|\.\.') { throw 'Unsafe runtime manifest entry' }
    $runtimeFile = Join-Path $runtime "bin/$($file.name)"
    if (!(Test-Path -LiteralPath $runtimeFile -PathType Leaf) -or (Get-FileHash -LiteralPath $runtimeFile -Algorithm SHA256).Hash -ne $file.sha256) {
        throw "Prepared runtime differs from the pinned build: $($file.name). Reprepare intentionally or replace LGPL components after making the local copy."
    }
}
New-Item -ItemType Directory -Path $destinationPath | Out-Null
Copy-Item -LiteralPath $executablePath -Destination (Join-Path $destinationPath $applicationName)
# A parallel build may replace the source after the preflight. Author the
# marker from the actual copied artifact, and never bless a stale preflight.
$buildMode = Read-DesktopEmbeddedBuildInfo -Executable (Join-Path $destinationPath $applicationName) -ExpectedProfile $expectedProfile
Copy-Item -LiteralPath $sidecar -Destination (Join-Path $destinationPath 'control-cli.exe')
$buildMode | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $destinationPath 'desktop-build-mode.json') -Encoding utf8NoBOM
Copy-Item -LiteralPath $runtime -Destination (Join-Path $destinationPath 'ffmpeg') -Recurse
Assert-FfmpegAudioDistribution -Root (Join-Path $destinationPath 'ffmpeg') | Out-Null
$notice = @'
AudioProcess portable application

Run the included application EXE. Keep control-cli.exe and the complete ffmpeg
folder beside it. No local Vite server or separately configured FFmpeg is needed.
Windows WebView2 Runtime is a prerequisite of the Tauri desktop shell.

The ffmpeg folder includes matching FFmpeg/LAME sources in
ffmpeg/sources/ffmpeg-audio-sources.zip, original licenses and NOTICE.txt.
Preserve these materials with the binary when sharing this folder.
Compatible modified FFmpeg/LAME shared libraries may be substituted.

This package operation verifies the FFmpeg component materials. It is not
a licensing/security audit of every AudioProcess component or user plugin.
'@
if ($LocalDevelopmentOnly) { $notice = "LOCAL DEBUG DEVELOPMENT BUILD`n`n" + $notice }
[IO.File]::WriteAllText((Join-Path $destinationPath 'README.txt'), $notice, [Text.UTF8Encoding]::new($false))
Write-Host "Created $expectedProfile application copy with FFmpeg sources and notices: $destinationPath"
