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
if (!(Test-Path -LiteralPath $provenancePath -PathType Leaf)) { throw 'FFmpeg resources missing; run scripts/prepare-ffmpeg.ps1 -RuntimeOnly first.' }
$provenance = Get-Content -LiteralPath $provenancePath -Raw | ConvertFrom-Json
if (!$LocalDevelopmentOnly) {
    if (!$provenance.redistribution_ready) {
        throw 'Distribution blocked: matching dependency sources and license review are incomplete. Only an explicitly requested local development copy is supported.'
    }
    throw 'A formal distributor must review source correspondence and implement an accompanying source package before enabling this release entry point.'
}
if (!$Executable) { $Executable = Join-Path $repository 'apps/desktop/src-tauri/target/debug/audioprocess-prototype.exe' }
if (!$Destination) { $Destination = Join-Path $repository 'build/AudioProcess-local-development' }
$executablePath = (Resolve-Path -LiteralPath $Executable).Path
$destinationPath = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $destinationPath) { throw 'Local development destination already exists; choose a new directory. Nothing was overwritten.' }
$sidecar = Join-Path (Split-Path -Parent $executablePath) 'control-cli.exe'
if (!(Test-Path -LiteralPath $sidecar -PathType Leaf)) { throw 'Required control-cli.exe sidecar is missing; no local copy was created.' }
$buildModePath = Join-Path (Split-Path -Parent $executablePath) 'desktop-build-mode.json'
if (!(Test-Path -LiteralPath $buildModePath -PathType Leaf)) { throw 'Build mode marker is missing. First run npm run tauri -- build --debug --no-bundle from apps/desktop.' }
$buildMode = Get-Content -LiteralPath $buildModePath -Raw | ConvertFrom-Json
if (!$buildMode.frontend_embedded -or $buildMode.profile -ne 'debug' -or (Get-Item -LiteralPath $executablePath).LastWriteTimeUtc -lt (Get-Item -LiteralPath $buildModePath).LastWriteTimeUtc) {
    throw 'An embedded-frontend debug application is required. First run npm run tauri -- build --debug --no-bundle from apps/desktop; ordinary cargo build is not accepted.'
}
foreach ($file in $provenance.files) {
    if ($file.name -match '[/\\:]|^\.|\.\.') { throw 'Unsafe runtime manifest entry' }
    $runtimeFile = Join-Path $runtime "bin/$($file.name)"
    if (!(Test-Path -LiteralPath $runtimeFile -PathType Leaf) -or (Get-FileHash -LiteralPath $runtimeFile -Algorithm SHA256).Hash -ne $file.sha256) {
        throw "Prepared runtime differs from the pinned build: $($file.name). Reprepare intentionally or replace LGPL components after making the local copy."
    }
}
New-Item -ItemType Directory -Path $destinationPath | Out-Null
Copy-Item -LiteralPath $executablePath -Destination (Join-Path $destinationPath 'AudioProcess-local-development.exe')
Copy-Item -LiteralPath $sidecar -Destination (Join-Path $destinationPath 'control-cli.exe')
Copy-Item -LiteralPath $buildModePath -Destination (Join-Path $destinationPath 'desktop-build-mode.json')
Copy-Item -LiteralPath $runtime -Destination (Join-Path $destinationPath 'ffmpeg') -Recurse
$notice = @'
LOCAL DEVELOPMENT COPY — DO NOT REDISTRIBUTE

Matching complete FFmpeg dependency sources and third-party notice review are
not available in this development copy. redistribution_ready=false remains in
ffmpeg/bundle-provenance.json. Standard release/distribution entry points are blocked.

This debug executable was prepared by Tauri CLI with its frontend embedded.
It does not require a local Vite development server.
FFmpeg and its shared libraries are included; the application does not need a
PATH FFmpeg installation. Compatible modified LGPL components may be substituted.

This warning does not claim that copying a debug executable can technically be
prevented. It records the incomplete distribution status of this local copy.
'@
[IO.File]::WriteAllText((Join-Path $destinationPath 'LOCAL-DEVELOPMENT-NOT-FOR-DISTRIBUTION.txt'), $notice, [Text.UTF8Encoding]::new($false))
Write-Host "Created local development copy: $destinationPath"
