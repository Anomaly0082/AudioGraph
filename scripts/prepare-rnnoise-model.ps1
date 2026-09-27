# The official model is a local build dependency, not redistributed in this repository.
# Its separate weights license is not yet clarified upstream (xiph/rnnoise#284).
[CmdletBinding()]
param([switch]$AcknowledgeUnclearModelLicense)
$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$modelRoot = Join-Path $projectRoot 'third_party/rnnoise/src'
$expected = @{
    'rnnoise_data.c' = '522b6a64fded05bf85e58c06206eafe57ce7d94f3af58c725b17628b481d7890'
    'rnnoise_data.h' = '09ff880bddd0fc74a2ae0e5ec6c8d65714031b08d0c3f672493acd9e189c5855'
}
$missing = @()
foreach ($name in $expected.Keys) {
    $target = Join-Path $modelRoot $name
    if (Test-Path -LiteralPath $target) {
        if (!(Test-Path -LiteralPath $target -PathType Leaf) -or
            (Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash -ne $expected[$name]) {
            throw "Existing model file does not match the pinned checksum; not overwriting: $target"
        }
    } else { $missing += $name }
}
if ($missing.Count -eq 0) {
    Write-Output 'RNNoise model 0b50c45 is present and both SHA256 hashes match. No network request.'
    return
}
if (!$AcknowledgeUnclearModelLicense) {
    throw 'Model weights license needs upstream clarification. For an explicitly chosen local experiment, rerun with -AcknowledgeUnclearModelLicense. This flag is not a redistribution license.'
}
$cacheRoot = Join-Path $projectRoot 'build/dependencies'
New-Item -ItemType Directory -Force -Path $cacheRoot | Out-Null
$archive = Join-Path $cacheRoot 'rnnoise_data-0b50c45.tar.gz'
$archiveHash = '4ac81c5c0884ec4bd5907026aaae16209b7b76cd9d7f71af582094a2f98f4b43'
if (!(Test-Path -LiteralPath $archive)) {
    # A unique download avoids overwriting an existing or partially downloaded artifact.
    $download = Join-Path $cacheRoot ('rnnoise-download-' + [Guid]::NewGuid().ToString('N') + '.tar.gz')
    Invoke-WebRequest -Uri 'https://media.xiph.org/rnnoise/models/rnnoise_data-0b50c45.tar.gz' -OutFile $download -TimeoutSec 120
    if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne $archiveHash) {
        throw 'Downloaded RNNoise model archive checksum mismatch; no model files installed.'
    }
    [IO.File]::Move($download, $archive)
}
if ((Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash -ne $archiveHash) {
    throw 'Cached RNNoise archive checksum mismatch; no model files installed.'
}
$stage = Join-Path $cacheRoot ('rnnoise-stage-' + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $stage | Out-Null
# Only these two known members from the verified archive are extracted; no training checkpoint.
& tar -xzf $archive -C $stage src/rnnoise_data.c src/rnnoise_data.h
if ($LASTEXITCODE -ne 0) { throw 'Could not extract RNNoise model.' }
foreach ($name in $expected.Keys) {
    if ((Get-FileHash -LiteralPath (Join-Path $stage "src/$name") -Algorithm SHA256).Hash -ne $expected[$name]) {
        throw 'Extracted RNNoise model checksum mismatch; no model files installed.'
    }
}
foreach ($name in $missing) {
    [IO.File]::Copy((Join-Path $stage "src/$name"), (Join-Path $modelRoot $name), $false)
}
Write-Output 'Prepared pinned RNNoise model for local builds. Weights remain Git-ignored; redistribution permission is not established.'
