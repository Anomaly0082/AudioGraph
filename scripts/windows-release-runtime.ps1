#requires -Version 7.0
# Only VS's unmodified x64 release CRT is copied, never debug_nonredist/System32.
function Find-ReleaseVcRuntime {
    param([string]$Directory)
    if ($Directory) { $candidates = @($Directory) }
    else {
        $candidates = @()
        if ($env:VCToolsRedistDir) { $candidates += Join-Path $env:VCToolsRedistDir 'x64/Microsoft.VC143.CRT' }
        $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
        if (Test-Path -LiteralPath $vswhere) {
            $installation = & $vswhere -latest -version '[17.0,18.0)' -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
            if ($installation) {
                $versions = @(Get-ChildItem -LiteralPath (Join-Path $installation 'VC/Redist/MSVC') -Directory | Where-Object Name -Match '^\d+\.\d+\.\d+$' | Sort-Object { [version]$_.Name } -Descending)
                $candidates += @($versions | ForEach-Object { Join-Path $_.FullName 'x64/Microsoft.VC143.CRT' })
            }
        }
    }
    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Container) {
            $resolved = (Resolve-Path -LiteralPath $candidate).Path
            if ($resolved -match '(?i)debug_nonredist|[/\\]system32([/\\]|$)' -or $resolved -notmatch '(?i)[/\\]VC[/\\]Redist[/\\]MSVC[/\\]\d+\.\d+\.\d+([.]\d+)?[/\\]x64[/\\]Microsoft\.VC143\.CRT$') {
                throw 'Use the x64 Microsoft.VC143.CRT folder from Visual Studio Redist, not debug or system files.'
            }
            $ancestor = Get-Item -LiteralPath $resolved
            while ($ancestor) {
                if ($ancestor.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Linked CRT source directory is unsupported.' }
                $ancestor = $ancestor.Parent
            }
            return $resolved
        }
    }
    throw 'VS 2022 x64 redistributable CRT is missing. Pass -VcRedistDirectory to its Microsoft.VC143.CRT folder.'
}

function Copy-ReleaseVcRuntime {
    param([Parameter(Mandatory)][string]$Directory, [Parameter(Mandatory)][string]$Destination)
    $root = Find-ReleaseVcRuntime -Directory $Directory
    $allowed = @('concrt140.dll','msvcp140.dll','msvcp140_1.dll','msvcp140_2.dll','msvcp140_atomic_wait.dll','msvcp140_codecvt_ids.dll',
        'vccorlib140.dll','vcruntime140.dll','vcruntime140_1.dll','vcruntime140_threads.dll')
    foreach ($required in @('msvcp140.dll','vcruntime140.dll','vcruntime140_1.dll')) {
        if (!(Test-Path -LiteralPath (Join-Path $root $required) -PathType Leaf)) { throw "Release CRT is incomplete: $required" }
    }
    $files = @(foreach ($name in $allowed) {
        $source = Join-Path $root $name
        if (!(Test-Path -LiteralPath $source -PathType Leaf)) { continue }
        $item = Get-Item -LiteralPath $source
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Linked CRT file is unsupported: $name" }
        $target = Join-Path $Destination $name
        if (Test-Path -LiteralPath $target) { throw "Refusing to replace existing package file: $name" }
        $hash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash.ToLowerInvariant()
        [IO.File]::Copy($source, $target, $false)
        if ((Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() -cne $hash) { throw "CRT copy mismatch: $name" }
        [ordered]@{name=$name;bytes=$item.Length;version=$item.VersionInfo.FileVersion;sha256=$hash}
    })
    $notice = @'
Microsoft Visual C++ 2022 x64 Runtime

These unmodified release runtime DLLs come from the licensed Visual Studio
installation's VC/Redist/MSVC/<version>/x64/Microsoft.VC143.CRT directory.
They remain subject to Microsoft's software license terms and are not covered
by AudioGraph or FFmpeg's open-source licenses. No debug_nonredist files are included.

Redistributable list and applicable conditions:
https://learn.microsoft.com/en-us/visualstudio/releases/2022/redistribution
Deployment and servicing guidance:
https://learn.microsoft.com/en-us/cpp/windows/redistributing-visual-cpp-files

This portable build uses application-local deployment. Update these DLLs from
the official release redistributable directory when maintaining the package;
they do not receive independent central servicing through this ZIP.
'@
    [IO.File]::WriteAllText((Join-Path $Destination 'MICROSOFT-CRT-NOTICE.txt'), $notice, [Text.UTF8Encoding]::new($false))
    return [ordered]@{schema_version=1;architecture='x64';source_kind='Visual Studio 2022 Redist release CRT';files=$files}
}
