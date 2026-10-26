#requires -Version 7.0
# Synthetic manifest tests only; no application, codec or user file is executed.
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'ffmpeg-audio-distribution.ps1')
$root = Join-Path ([IO.Path]::GetTempPath()) ('ffmpeg-material-test-' + [guid]::NewGuid().ToString('N'))
$names = @('bin/ffmpeg.exe','bin/ffprobe.exe','bin/libmp3lame-0.dll','bin/avcodec-62.dll','bin/avformat-62.dll','bin/avfilter-11.dll','bin/avutil-60.dll','bin/swresample-6.dll',
    'NOTICE.txt','sources/ffmpeg-audio-sources.zip','licenses/FFmpeg/COPYING.LGPLv2.1','licenses/FFmpeg/COPYING.GPLv2','licenses/FFmpeg/LICENSE.md','licenses/FFmpeg/CREDITS',
    'licenses/LAME/COPYING','licenses/LAME/LICENSE','licenses/GCC/COPYING.RUNTIME','licenses/GCC/COPYING3','licenses/GCC/COPYING.LIB','licenses/GCC/README',
    'licenses/MinGW/COPYING','licenses/MinGW/COPYING.MinGW-w64-runtime.txt','licenses/MinGW/COPYING.MinGW-w64.txt','licenses/winpthreads/COPYING')
$checks = 0
function Write-Manifest($entries) {
    @{schema=2;provider='AudioGraph audio-only build';redistribution_ready=$true;distribution_files=$entries} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $root 'bundle-provenance.json')
}
function Reject-Invalid {
    $rejected = $false
    try { Assert-FfmpegAudioDistribution -Root $root | Out-Null } catch { $rejected = $true }
    if (!$rejected) { throw 'Invalid FFmpeg distribution was accepted.' }
    $script:checks++
}
try {
    $entries = @(foreach ($name in $names) {
        $path = Join-Path $root $name
        New-Item -ItemType Directory -Path (Split-Path -Parent $path) -Force | Out-Null
        [IO.File]::WriteAllBytes($path, [Text.Encoding]::UTF8.GetBytes('fixture'))
        @{path=$name;bytes=7;sha256=(Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()}
    })
    Write-Manifest $entries
    Assert-FfmpegAudioDistribution -Root $root | Out-Null
    $checks++
    $notice = Join-Path $root 'NOTICE.txt'
    [IO.File]::WriteAllText($notice, 'changed')
    Reject-Invalid
    [IO.File]::WriteAllText($notice, 'fixture')
    $source = Join-Path $root 'sources/ffmpeg-audio-sources.zip'
    Remove-Item -LiteralPath $source
    Reject-Invalid
    [IO.File]::WriteAllText($source, 'fixture')
    $extra = Join-Path $root 'bin/swscale-legacy.dll'
    [IO.File]::WriteAllText($extra, 'unused legacy file')
    Reject-Invalid
    Remove-Item -LiteralPath $extra
    $missingNotice = @($entries | Where-Object path -NE 'licenses/LAME/LICENSE')
    Remove-Item -LiteralPath (Join-Path $root 'licenses/LAME/LICENSE')
    Write-Manifest $missingNotice
    Reject-Invalid
    Write-Manifest ($entries + $entries[0])
    Reject-Invalid
    Write-Host "FFmpeg distribution materials: $checks checks passed; no executable launched."
} finally {
    $resolved = [IO.Path]::GetFullPath($root)
    $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temp,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notmatch '^ffmpeg-material-test-[0-9a-f]{32}$') { throw 'Unexpected cleanup target.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
