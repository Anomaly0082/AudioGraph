#requires -Version 7.0
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'nsis-build-info.ps1')
$root = Join-Path ([IO.Path]::GetTempPath()) ('nsis-marker-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$file = Join-Path $root 'fixture.bin'
$prefix = '__TAURI_BUNDLE_TYPE_VAR_'
$checks = 0
try {
    [IO.File]::WriteAllBytes($file,[Text.Encoding]::ASCII.GetBytes("MZ${prefix}UNK-tail"))
    $sourceHash = (Get-FileHash -LiteralPath $file).Hash.ToLowerInvariant()
    $record = Get-NsisExpectedImage -Executable $file
    $expectedBytes = [Text.Encoding]::ASCII.GetBytes("MZ${prefix}NSS-tail")
    $expectedHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($expectedBytes)).ToLowerInvariant()
    if ($record.sha256 -cne $expectedHash -or $record.input_sha256 -cne $sourceHash -or (Get-FileHash -LiteralPath $file).Hash.ToLowerInvariant() -cne $sourceHash) { throw 'Marker prediction changed source or wrong bytes.' }
    $checks++
    [IO.File]::WriteAllBytes($file,$expectedBytes)
    if ((Get-NsisExpectedImage -Executable $file).sha256 -cne $expectedHash) { throw 'NSS input must be idempotent.' }
    $checks++
    foreach ($text in @('no marker',"${prefix}MSI", "${prefix}UNK${prefix}UNK", "${prefix}UN")) {
        [IO.File]::WriteAllText($file,$text)
        $rejected = $false
        try { Get-NsisExpectedImage -Executable $file | Out-Null } catch { $rejected = $true }
        if (!$rejected) { throw 'Invalid marker was accepted.' }
        $checks++
    }
    Write-Host "NSIS bundle-kind prediction: $checks checks passed; no binary executed or modified."
} finally {
    $resolved = [IO.Path]::GetFullPath($root)
    $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temp,[StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notmatch '^nsis-marker-test-[0-9a-f]{32}$') { throw 'Unexpected cleanup path.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
