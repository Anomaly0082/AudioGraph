#requires -Version 7.0
# Synthetic files only; no DLL or application is executed.
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'windows-release-runtime.ps1')
$root = Join-Path ([IO.Path]::GetTempPath()) ('release-crt-test-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $root | Out-Null
$script:checks = 0
function Reject([scriptblock]$Action) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    if (!$rejected) { throw 'Invalid CRT operation was accepted.' }
    $script:checks++
}
try {
    $source = Join-Path $root 'VC/Redist/MSVC/14.44.35211/x64/Microsoft.VC143.CRT'
    New-Item -ItemType Directory -Path $source -Force | Out-Null
    $required = @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')
    foreach ($name in $required) { [IO.File]::WriteAllText((Join-Path $source $name), "synthetic-$name") }
    [IO.File]::WriteAllText((Join-Path $source 'vcruntime140d.dll'), 'debug must not be copied')
    [IO.File]::WriteAllText((Join-Path $source 'unexpected.dll'), 'extra must not be copied')
    $output = Join-Path $root 'package'
    New-Item -ItemType Directory -Path $output | Out-Null
    $record = Copy-ReleaseVcRuntime -Directory $source -Destination $output
    if ($record.schema_version -ne 1 -or $record.architecture -cne 'x64' -or @($record.files).Count -ne 3) { throw 'Incorrect CRT manifest.' }
    foreach ($entry in $record.files) {
        if ($entry.sha256 -cne (Get-FileHash -LiteralPath (Join-Path $output $entry.name) -Algorithm SHA256).Hash.ToLowerInvariant()) { throw 'Incorrect CRT hash.' }
    }
    if ((Test-Path -LiteralPath (Join-Path $output 'unexpected.dll')) -or (Test-Path -LiteralPath (Join-Path $output 'vcruntime140d.dll'))) { throw 'Unapproved file copied.' }
    if (!(Test-Path -LiteralPath (Join-Path $output 'MICROSOFT-CRT-NOTICE.txt'))) { throw 'Missing CRT notice.' }
    $script:checks++
    Reject { Copy-ReleaseVcRuntime -Directory $source -Destination $output }
    foreach ($unsafe in @('debug_nonredist/x64/Microsoft.VC143.CRT','System32/x64/Microsoft.VC143.CRT','x86/Microsoft.VC143.CRT',
        'fake/x64/Microsoft.VC143.CRT','VC/Redist/MSVC/not-a-version/x64/Microsoft.VC143.CRT')) {
        $path = Join-Path $root $unsafe
        New-Item -ItemType Directory -Path $path -Force | Out-Null
        Reject { Find-ReleaseVcRuntime -Directory $path }
    }
    $incomplete = Join-Path $root 'incomplete'
    New-Item -ItemType Directory -Path $incomplete | Out-Null
    Remove-Item -LiteralPath (Join-Path $source 'vcruntime140_1.dll')
    Reject { Copy-ReleaseVcRuntime -Directory $source -Destination $incomplete }
    if (@(Get-ChildItem -LiteralPath $incomplete -Force).Count -ne 0) { throw 'Incomplete source copied partial files.' }
    Write-Host "Release CRT synthetic fixtures: $script:checks checks passed; no DLL executed."
} finally {
    $resolved = [IO.Path]::GetFullPath($root)
    $temp = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd('\') + '\'
    if (!$resolved.StartsWith($temp, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolved) -notmatch '^release-crt-test-[0-9a-f]{32}$') { throw 'Unexpected cleanup target.' }
    Remove-Item -LiteralPath $resolved -Recurse -Force
}
