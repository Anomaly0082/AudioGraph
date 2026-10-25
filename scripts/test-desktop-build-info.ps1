#requires -Version 7.0
[CmdletBinding()]
param(
    # Optional artifacts built serially by the caller. Neither is executed.
    [string]$EmbeddedExecutable,
    [string]$DevExecutable
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'desktop-build-info.ps1')
$fixture = Join-Path ([IO.Path]::GetTempPath()) ('desktop-build-info-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $fixture | Out-Null
$begin = 'AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_BEGIN'
$end = 'AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_END'
$valid = '{"schema_version":1,"profile":"debug","frontend_embedded":true}'
$script:checks = 0
function Assert-Rejected {
    param([string]$Path, [string]$Name)
    $rejected = $false
    try { $null = Read-DesktopEmbeddedBuildInfo -Executable $Path } catch { $rejected = $true }
    if (!$rejected) { throw "Expected rejection: $Name" }
    $script:checks++
}
function Write-Image {
    param([string]$Name, [string]$Content)
    $path = Join-Path $fixture $Name
    [IO.File]::WriteAllBytes($path, [Text.Encoding]::GetEncoding(28591).GetBytes("MZ`0`0$Content`0binary-tail"))
    return $path
}
try {
    $path = Write-Image 'valid.exe' "$begin$valid$end"
    $mode = Read-DesktopEmbeddedBuildInfo -Executable $path
    if ($mode.schema_version -ne 1 -or $mode.profile -cne 'debug' -or $mode.frontend_embedded -ne $true) { throw 'Valid embedded debug metadata was not preserved.' }
    $script:checks++
    $cases = [ordered]@{
        missing = 'MZ-no-build-info'
        missing_end = "$begin$valid"
        missing_begin = "$valid$end"
        multiple = "$begin$valid$end$begin$valid$end"
        duplicate_begin = "$begin$begin$valid$end"
        duplicate_end = "$begin$valid$end$end"
        reversed = "$end$valid$begin"
        malformed = "$begin{bad json}$end"
        array = "$begin[]$end"
        release = "$begin" + '{"schema_version":1,"profile":"release","frontend_embedded":true}' + $end
        dev = "$begin" + '{"schema_version":1,"profile":"debug","frontend_embedded":false}' + $end
        string_bool = "$begin" + '{"schema_version":1,"profile":"debug","frontend_embedded":"true"}' + $end
        numeric_bool = "$begin" + '{"schema_version":1,"profile":"debug","frontend_embedded":1}' + $end
        schema_string = "$begin" + '{"schema_version":"1","profile":"debug","frontend_embedded":true}' + $end
        future_schema = "$begin" + '{"schema_version":2,"profile":"debug","frontend_embedded":true}' + $end
        floating_schema = "$begin" + '{"schema_version":1.0,"profile":"debug","frontend_embedded":true}' + $end
        duplicate_field = "$begin" + '{"schema_version":1,"profile":"release","profile":"debug","frontend_embedded":true}' + $end
        unknown_field = "$begin" + '{"schema_version":1,"profile":"debug","frontend_embedded":true,"extra":1}' + $end
        wrong_case = "$begin" + '{"schema_version":1,"Profile":"debug","frontend_embedded":true}' + $end
        incomplete = "$begin" + '{"schema_version":1,"profile":"debug"}' + $end
        oversized = "$begin" + (' ' * 513) + $valid + $end
        trailing_json = "$begin$valid{}$end"
    }
    foreach ($case in $cases.GetEnumerator()) { Assert-Rejected (Write-Image "$($case.Key).exe" $case.Value) $case.Key }
    # This sidecar deliberately contradicts the artifact, and has a later mtime.
    $marker = Join-Path $fixture 'desktop-build-mode.json'
    [IO.File]::WriteAllText($marker, '{"profile":"debug","frontend_embedded":false}')
    [IO.File]::SetLastWriteTimeUtc($marker, [DateTime]::UtcNow.AddDays(1))
    $null = Read-DesktopEmbeddedBuildInfo -Executable $path
    $script:checks++
    if ($EmbeddedExecutable) {
        $realMode = Read-DesktopEmbeddedBuildInfo -Executable (Resolve-Path -LiteralPath $EmbeddedExecutable).Path
        $source = Join-Path $fixture 'real-artifact'
        New-Item -ItemType Directory -Path $source | Out-Null
        $realCopy = Join-Path $source 'embedded.exe'
        Copy-Item -LiteralPath $EmbeddedExecutable -Destination $realCopy
        $realSidecar = Join-Path (Split-Path -Parent (Resolve-Path -LiteralPath $EmbeddedExecutable).Path) 'control-cli.exe'
        Copy-Item -LiteralPath $realSidecar -Destination (Join-Path $source 'control-cli.exe')
        Copy-Item -LiteralPath $marker -Destination (Join-Path $source 'desktop-build-mode.json')
        $package = Join-Path $fixture 'local-package'
        & (Join-Path $PSScriptRoot 'package-desktop.ps1') -LocalDevelopmentOnly -Executable $realCopy -Destination $package
        $packagedMode = Get-Content -LiteralPath (Join-Path $package 'desktop-build-mode.json') -Raw | ConvertFrom-Json
        if ($packagedMode.schema_version -ne 1 -or $packagedMode.frontend_embedded -ne $true -or $packagedMode.profile -cne 'debug') { throw 'Packaged marker did not come from the actual embedded executable.' }
        if ((Get-FileHash -LiteralPath $realCopy).Hash -ne (Get-FileHash -LiteralPath (Join-Path $package 'AudioProcess-local-development.exe')).Hash) { throw 'Packaged executable changed.' }
        $script:checks++
    }
    if ($DevExecutable) {
        Assert-Rejected (Resolve-Path -LiteralPath $DevExecutable).Path 'actual ordinary cargo build executable'
    }
    Write-Host "Desktop embedded build metadata: $script:checks checks passed. No executable was launched."
} finally {
    # Only remove this test's concrete, randomly named temporary directory.
    $resolvedFixture = [IO.Path]::GetFullPath($fixture)
    $temporaryRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath()).TrimEnd([IO.Path]::DirectorySeparatorChar) + [IO.Path]::DirectorySeparatorChar
    if (!$resolvedFixture.StartsWith($temporaryRoot, [StringComparison]::OrdinalIgnoreCase) -or (Split-Path -Leaf $resolvedFixture) -notmatch '^desktop-build-info-[0-9a-f]{32}$') { throw 'Refusing unexpected test cleanup path.' }
    Remove-Item -LiteralPath $resolvedFixture -Recurse -Force
}
