[CmdletBinding()]
param(
    [string]$CacheDirectory,
    [switch]$RuntimeOnly,
    [switch]$Offline,
    [switch]$Force
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path -Parent $PSScriptRoot
if (!$CacheDirectory) { $CacheDirectory = Join-Path $repository 'build/ffmpeg-cache' }
$cache = [IO.Path]::GetFullPath($CacheDirectory)
$runtime = Join-Path $repository 'apps/desktop/src-tauri/resources/ffmpeg'
$lockPath = Join-Path $PSScriptRoot 'ffmpeg-bundle.lock.json'
$pin = Get-Content -LiteralPath $lockPath -Raw | ConvertFrom-Json
New-Item -ItemType Directory -Path $cache -Force | Out-Null

function Assert-Hash([string]$Path, [string]$Expected) {
    if (!(Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Required file is missing: $Path" }
    $actual = (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $Expected.ToLowerInvariant()) { throw "SHA-256 mismatch: $Path" }
}
function Get-PinnedArchive($Asset) {
    $destination = Join-Path $cache $Asset.name
    if (!(Test-Path -LiteralPath $destination -PathType Leaf)) {
        if ($Offline) { throw "Offline cache is incomplete: $destination" }
        $temporary = "$destination.partial-$([guid]::NewGuid().ToString('N'))"
        try {
            Invoke-WebRequest -Uri $Asset.url -OutFile $temporary
            Assert-Hash $temporary $Asset.sha256
            Move-Item -LiteralPath $temporary -Destination $destination
        } finally { if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary } }
    }
    Assert-Hash $destination $Asset.sha256
    return $destination
}
function Copy-ZipEntry([string]$Archive, [string]$Entry, [string]$Destination) {
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        $item = $zip.GetEntry($Entry)
        if (!$item) { throw "Missing archive entry: $Entry" }
        $inputStream = $item.Open()
        $outputStream = [IO.File]::Create($Destination)
        try { $inputStream.CopyTo($outputStream) } finally { $outputStream.Dispose(); $inputStream.Dispose() }
    } finally { $zip.Dispose() }
}
function Assert-SafeArchive([string]$Archive) {
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($entry in $zip.Entries) {
            $name = $entry.FullName.Replace('\', '/')
            if ($name.StartsWith('/') -or $name.Contains(':') -or $name.Split('/') -contains '..') {
                throw 'Archive contains an unsafe path'
            }
        }
    } finally { $zip.Dispose() }
}

$binary = Get-PinnedArchive $pin.binary
$ffmpegSource = Get-PinnedArchive $pin.ffmpeg_source
$builderSource = Get-PinnedArchive $pin.builder_source
$dependencySources = Join-Path $cache $pin.dependency_sources.name
$sourcesReady = Test-Path -LiteralPath $dependencySources -PathType Leaf
if ($sourcesReady) { Assert-Hash $dependencySources $pin.dependency_sources.sha256 }
if (!$RuntimeOnly -and !$sourcesReady) {
    throw @"
Matching dependency sources are missing. Distribution preparation is blocked.
Download the official authenticated Actions artifact without changing its ZIP bytes:
https://github.com/BtbN/FFmpeg-Builds/actions/runs/39776938430/artifacts/12424698518
Place it at: $dependencySources
Expected SHA-256: $($pin.dependency_sources.sha256)
For local development only, use -RuntimeOnly. Do not redistribute that incomplete package.
"@
}

$stage = Join-Path $cache "prepare-$([guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $stage | Out-Null
try {
    Assert-SafeArchive $binary
    $unpack = Join-Path $stage 'unpack'
    Expand-Archive -LiteralPath $binary -DestinationPath $unpack
    $packages = @(Get-ChildItem -LiteralPath $unpack -Directory)
    if ($packages.Count -ne 1) { throw 'Unexpected FFmpeg archive layout' }
    $package = $packages[0].FullName
    $prepared = Join-Path $stage 'ffmpeg'
    $bin = Join-Path $prepared 'bin'
    $licenses = Join-Path $prepared 'licenses'
    New-Item -ItemType Directory -Path $bin,$licenses -Force | Out-Null
    foreach ($name in @('ffmpeg.exe','ffprobe.exe','avcodec-62.dll','avdevice-62.dll','avfilter-11.dll','avformat-62.dll','avutil-60.dll','swresample-6.dll','swscale-9.dll')) {
        Copy-Item -LiteralPath (Join-Path $package "bin/$name") -Destination (Join-Path $bin $name)
    }
    Copy-Item -LiteralPath (Join-Path $package 'LICENSE.txt') -Destination (Join-Path $licenses 'FFmpeg-LGPL-3.0.txt')
    Copy-ZipEntry $ffmpegSource "FFmpeg-$($pin.ffmpeg_revision)/COPYING.GPLv3" (Join-Path $licenses 'GPL-3.0.txt')
    Copy-ZipEntry $ffmpegSource "FFmpeg-$($pin.ffmpeg_revision)/LICENSE.md" (Join-Path $licenses 'FFmpeg-LICENSE.md')
    Copy-ZipEntry $builderSource "FFmpeg-Builds-$($pin.builder_commit)/LICENSE" (Join-Path $licenses 'BtbN-build-scripts-MIT.txt')
    Copy-Item -LiteralPath (Join-Path $repository 'docs/ffmpeg-bundle-NOTICE.txt') -Destination (Join-Path $prepared 'NOTICE.txt')
    Copy-Item -LiteralPath $lockPath -Destination (Join-Path $prepared 'upstream-lock.json')
    $ffmpeg = Join-Path $bin 'ffmpeg.exe'
    $licenseOutput = (& $ffmpeg -hide_banner -L 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0 -or $licenseOutput -notmatch 'Lesser General Public License' -or $licenseOutput -notmatch 'version 3') {
        throw 'Bundled executable did not report the expected LGPL version 3 license'
    }
    $buildConfiguration = (& $ffmpeg -hide_banner -buildconf 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0 -or $buildConfiguration -match '--enable-gpl|--enable-nonfree' -or $buildConfiguration -notmatch '--enable-shared') {
        throw 'Unexpected FFmpeg build configuration; refuse GPL/nonfree or non-shared builds'
    }
    [IO.File]::WriteAllText((Join-Path $prepared 'build-configuration.txt'), $buildConfiguration, [Text.UTF8Encoding]::new($false))
    [IO.File]::WriteAllText((Join-Path $prepared 'license-report.txt'), $licenseOutput, [Text.UTF8Encoding]::new($false))
    $files = @(Get-ChildItem -LiteralPath $bin -File | ForEach-Object {
        [ordered]@{ name=$_.Name; bytes=$_.Length; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant() }
    })
    $provenance = [ordered]@{
        schema=1; provider=$pin.provider; release=$pin.release; ffmpeg_revision=$pin.ffmpeg_revision
        builder_commit=$pin.builder_commit; variant=$pin.variant; license='LGPL-3.0-or-later'
        binary_archive_sha256=$pin.binary.sha256; files=$files
        dependency_sources_present=$sourcesReady; redistribution_ready=$false
        redistribution_note='Full source cache, dependency notices, and source correspondence review are required before distribution.'
        replacement_rights='Runtime does not enforce these hashes. Users may replace compatible FFmpeg programs and LGPL shared libraries.'
    }
    [IO.File]::WriteAllText((Join-Path $prepared 'bundle-provenance.json'), ($provenance | ConvertTo-Json -Depth 8), [Text.UTF8Encoding]::new($false))
    if (Test-Path -LiteralPath $runtime) {
        if (!$Force) { throw 'FFmpeg resources already exist. Use -Force to preserve the previous bundle in a backup and install the pinned build.' }
        $backup = "$runtime.previous-$([guid]::NewGuid().ToString('N'))"
        Move-Item -LiteralPath $runtime -Destination $backup
        Write-Host "Previous replaceable bundle preserved at $backup"
    }
    New-Item -ItemType Directory -Path (Split-Path -Parent $runtime) -Force | Out-Null
    Move-Item -LiteralPath $prepared -Destination $runtime
    Write-Host "Prepared local FFmpeg runtime: $runtime"
    if (!$sourcesReady) { Write-Warning 'Dependency sources are missing. Local development only; redistribution remains blocked.' }
} finally {
    # Exact fresh staging directory created above, never a workspace or user path.
    if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
}
