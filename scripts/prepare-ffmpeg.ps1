#requires -Version 7.0
[CmdletBinding()]
param(
    [string]$CacheDirectory,
    [string]$BuildDirectory,
    [string]$MsysRoot,
    [ValidateRange(1,4)][int]$Jobs = 4,
    [switch]$Offline,
    [switch]$Force,
    # Legacy commands remain accepted; sources and notices are always included.
    [switch]$RuntimeOnly
)
$ErrorActionPreference = 'Stop'
$repository = Split-Path -Parent $PSScriptRoot
if (!$CacheDirectory) { $CacheDirectory = Join-Path $repository 'build/ffmpeg-cache' }
if (!$BuildDirectory) { $BuildDirectory = Join-Path $repository 'build/ffmpeg-audio' }
if (!$MsysRoot) {
    $compiler = Get-Command gcc.exe -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
    if (!$compiler -or $compiler.Source -notmatch '[/\\]mingw64[/\\]bin[/\\]gcc.exe$') {
        throw 'Developer build requires MSYS2 MinGW64; pass -MsysRoot. End users do not need this toolchain.'
    }
    $MsysRoot = Split-Path (Split-Path (Split-Path $compiler.Source -Parent) -Parent) -Parent
}
$parameters = @{ MsysRoot=$MsysRoot; CacheDirectory=$CacheDirectory; BuildDirectory=$BuildDirectory; Jobs=$Jobs; Offline=$Offline }
& (Join-Path $PSScriptRoot 'build-ffmpeg-audio.ps1') @parameters
if ($LASTEXITCODE) { throw 'Audio-only FFmpeg build failed.' }
$arguments = @((Join-Path $PSScriptRoot 'prepare-ffmpeg-audio.py'), '--build-directory', $BuildDirectory, '--cache', $CacheDirectory, '--msys-root', $MsysRoot)
if ($Force) { $arguments += '--force' }
& python @arguments
if ($LASTEXITCODE) { throw 'Audio-only FFmpeg source/notice preparation failed.' }
. (Join-Path $PSScriptRoot 'ffmpeg-audio-distribution.ps1')
Assert-FfmpegAudioDistribution -Root (Join-Path $repository 'apps/desktop/src-tauri/resources/ffmpeg') | Out-Null
Write-Host 'Prepared bundled audio FFmpeg, matching sources and license notices. No end-user FFmpeg setup is required.'
