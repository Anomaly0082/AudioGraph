#requires -Version 7.0
# Preparation/package-time verification only. Runtime replacement stays unrestricted.
function Assert-FfmpegAudioDistribution {
    param([Parameter(Mandatory)][string]$Root)
    $metadata = Get-Content -LiteralPath (Join-Path $Root 'bundle-provenance.json') -Raw | ConvertFrom-Json
    if ($metadata.schema -ne 2 -or $metadata.provider -cne 'AudioGraph audio-only build' -or $metadata.redistribution_ready -isnot [bool] -or $metadata.redistribution_ready -ne $true) {
        throw 'The audio-only FFmpeg source/notice bundle is not prepared for distribution.'
    }
    $files = @($metadata.distribution_files)
    if ($files.Count -lt 10) { throw 'FFmpeg distribution manifest is incomplete.' }
    $seen = [Collections.Generic.HashSet[string]]::new([StringComparer]::OrdinalIgnoreCase)
    foreach ($entry in $files) {
        $relative = [string]$entry.path
        if (!$relative -or $relative -match '\\|:|^/|(^|/)\.\.?(/|$)' -or !$seen.Add($relative) -or $entry.sha256 -notmatch '^[0-9a-f]{64}$') {
            throw 'Invalid or duplicated FFmpeg distribution file entry.'
        }
        $path = Join-Path $Root $relative
        $item = Get-Item -LiteralPath $path -ErrorAction Stop
        if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw "Invalid distribution file: $relative" }
        $ancestor = $item.Directory
        while ($ancestor -and $ancestor.FullName.Length -ge ([IO.Path]::GetFullPath($Root)).Length) {
            if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Linked distribution directory is unsupported.' }
            $ancestor = $ancestor.Parent
        }
        if ($item.Length -ne $entry.bytes -or (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant() -cne $entry.sha256) {
            throw "FFmpeg source/runtime/notice file differs from its prepared manifest: $relative"
        }
    }
    foreach ($required in @('bin/ffmpeg.exe','bin/ffprobe.exe','bin/libmp3lame-0.dll','bin/avcodec-62.dll','bin/avformat-62.dll','bin/avfilter-11.dll','bin/avutil-60.dll','bin/swresample-6.dll',
        'NOTICE.txt','sources/ffmpeg-audio-sources.zip','licenses/FFmpeg/COPYING.LGPLv2.1','licenses/FFmpeg/COPYING.GPLv2','licenses/FFmpeg/LICENSE.md','licenses/FFmpeg/CREDITS',
        'licenses/LAME/COPYING','licenses/LAME/LICENSE','licenses/GCC/COPYING.RUNTIME','licenses/GCC/COPYING3','licenses/GCC/COPYING.LIB','licenses/GCC/README',
        'licenses/MinGW/COPYING','licenses/MinGW/COPYING.MinGW-w64-runtime.txt','licenses/MinGW/COPYING.MinGW-w64.txt','licenses/winpthreads/COPYING')) {
        if (!$seen.Contains($required)) { throw "Required FFmpeg distribution material absent: $required" }
    }
    foreach ($item in @((Get-Item -LiteralPath $Root)) + @(Get-ChildItem -LiteralPath $Root -Recurse -Force)) {
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Linked distribution entry is unsupported.' }
    }
    $actual = @(Get-ChildItem -LiteralPath $Root -Recurse -File -Force | ForEach-Object { [IO.Path]::GetRelativePath([IO.Path]::GetFullPath($Root), $_.FullName).Replace('\','/') })
    foreach ($name in $actual) {
        if ($name -cne 'bundle-provenance.json' -and !$seen.Contains($name)) { throw "Unlisted file in FFmpeg distribution: $name" }
    }
    return $metadata
}
