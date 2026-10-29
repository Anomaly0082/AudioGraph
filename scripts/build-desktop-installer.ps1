#requires -Version 7.0
[CmdletBinding()]
param([switch]$SkipBuild, [string]$Destination)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$repository = Split-Path -Parent $PSScriptRoot
$desktop = Join-Path $repository 'apps/desktop'
$native = Join-Path $desktop 'src-tauri'
$version = (Get-Content -LiteralPath (Join-Path $native 'tauri.conf.json') -Raw | ConvertFrom-Json).version
if (!$Destination) { $Destination = Join-Path $repository "build/AudioGraph_${version}_x64-setup.exe" }
$Destination = [IO.Path]::GetFullPath($Destination)
if (Test-Path -LiteralPath $Destination) { throw 'Installer destination exists; choose another path. Nothing overwritten.' }
$oldLocation = Get-Location
try {
    Set-Location -LiteralPath $repository
    if (!$SkipBuild) {
        & cmake --preset windows-msvc
        if ($LASTEXITCODE) { throw 'CMake configure failed.' }
        & cmake --build --preset release --target control-cli --parallel 4
        if ($LASTEXITCODE) { throw 'C++ Release build failed.' }
        Set-Location -LiteralPath $desktop
        & npm.cmd run tauri -- build --no-bundle
        if ($LASTEXITCODE) { throw 'Tauri Release build failed.' }
        Set-Location -LiteralPath $repository
    }
    $cli = Get-Content -LiteralPath (Join-Path $desktop 'node_modules/@tauri-apps/cli/package.json') -Raw | ConvertFrom-Json
    if ($cli.version -cne '2.11.4') { throw 'Review the customized NSIS template before upgrading the pinned Tauri CLI 2.11.4.' }
    $stage = Join-Path $repository ('build/installer-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $stage | Out-Null
    $payload = Join-Path $stage 'payload'
    & (Join-Path $PSScriptRoot 'package-desktop.ps1') -Destination $payload
    $main = Join-Path $native 'target/release/audioprocess-prototype.exe'
    . (Join-Path $PSScriptRoot 'nsis-build-info.ps1')
    $nsisMain = Get-NsisExpectedImage -Executable $main
    if ((Get-FileHash -LiteralPath $main).Hash -cne (Get-FileHash -LiteralPath (Join-Path $payload 'AudioProcess.exe')).Hash) { throw 'Main executable changed during staging.' }
    $external = Join-Path $native 'binaries/control-cli-x86_64-pc-windows-msvc.exe'
    if ((Get-FileHash -LiteralPath $external).Hash -cne (Get-FileHash -LiteralPath (Join-Path $payload 'control-cli.exe')).Hash) { throw 'Tauri externalBin is not the verified Release sidecar.' }

    # Bundle a small, signed Microsoft bootstrapper. It is never run by this build.
    $tools = Join-Path $native 'target/.tauri'
    New-Item -ItemType Directory -Path $tools -Force | Out-Null
    $bootstrap = Join-Path $tools 'MicrosoftEdgeWebview2Setup.exe'
    if (!(Test-Path -LiteralPath $bootstrap)) {
        Invoke-WebRequest -Uri 'https://go.microsoft.com/fwlink/p/?LinkId=2124703' -OutFile $bootstrap
    }
    $signature = Get-AuthenticodeSignature -LiteralPath $bootstrap
    if ($signature.Status -ne 'Valid' -or !$signature.SignerCertificate -or $signature.SignerCertificate.Subject -notmatch 'CN=Microsoft Corporation(,|$)') {
        throw 'WebView2 bootstrapper is not a valid Microsoft-signed file; installer not built.'
    }
    $bootstrapRecord = @{url='https://go.microsoft.com/fwlink/p/?LinkId=2124703';sha256=(Get-FileHash -LiteralPath $bootstrap).Hash;signer=$signature.SignerCertificate.Subject;bytes=(Get-Item -LiteralPath $bootstrap).Length}
    $bootstrapRecord | ConvertTo-Json | Set-Content -LiteralPath (Join-Path $stage 'webview2-bootstrapper.json') -Encoding utf8NoBOM

    # The base FFmpeg directory is verified above. Additional resource files come
    # from the same validated Release payload (CRT, notices and build metadata).
    $resources = @{}
    foreach ($file in Get-ChildItem -LiteralPath $payload -File) {
        if ($file.Name -in @('AudioProcess.exe','control-cli.exe')) { continue }
        $resources[$file.FullName.Replace('\','/')] = $file.Name
    }
    $resources[(Join-Path $native 'installer/LICENSE-MIT.txt').Replace('\','/')] = 'installer-licenses/Tauri-MIT.txt'
    $resources[(Join-Path $native 'installer/NSIS-COPYING.txt').Replace('\','/')] = 'installer-licenses/NSIS-COPYING.txt'
    $resources[(Join-Path $native 'installer/nsis-tauri-utils-MIT.txt').Replace('\','/')] = 'installer-licenses/nsis-tauri-utils-MIT.txt'
    $overlay = Join-Path $stage 'resources.json'
    @{bundle=@{resources=$resources}} | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $overlay -Encoding utf8NoBOM
    $expected = @(Get-ChildItem -LiteralPath $payload -Recurse -File | ForEach-Object {
        $relative = [IO.Path]::GetRelativePath($payload,$_.FullName).Replace('\','/')
        if ($relative -eq 'AudioProcess.exe') { $relative = 'audioprocess-prototype.exe' }
        @{path=$relative;sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant();bytes=$_.Length}
    })
    $templateLicense = Join-Path $native 'installer/LICENSE-MIT.txt'
    $expected += @{path='installer-licenses/Tauri-MIT.txt';sha256=(Get-FileHash -LiteralPath $templateLicense -Algorithm SHA256).Hash.ToLowerInvariant();bytes=(Get-Item -LiteralPath $templateLicense).Length}
    $nsisLicense = Join-Path $native 'installer/NSIS-COPYING.txt'
    $expected += @{path='installer-licenses/NSIS-COPYING.txt';sha256=(Get-FileHash -LiteralPath $nsisLicense -Algorithm SHA256).Hash.ToLowerInvariant();bytes=(Get-Item -LiteralPath $nsisLicense).Length}
    $pluginLicense = Join-Path $native 'installer/nsis-tauri-utils-MIT.txt'
    $expected += @{path='installer-licenses/nsis-tauri-utils-MIT.txt';sha256=(Get-FileHash -LiteralPath $pluginLicense -Algorithm SHA256).Hash.ToLowerInvariant();bytes=(Get-Item -LiteralPath $pluginLicense).Length}
    $expected | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stage 'expected-payload.json') -Encoding utf8NoBOM
    Set-Location -LiteralPath $desktop
    & npm.cmd run tauri -- bundle --bundles nsis --config src-tauri/tauri.installer.conf.json --config $overlay --ci --no-sign
    if ($LASTEXITCODE) { throw "NSIS build failed; logs/staging retained at $stage" }
    # Tauri patches UNK -> NSS while packing and restores the source afterwards.
    # Expected payload hash was predicted from the trusted source, not learned
    # from the extracted installer (which would make verification circular).
    . (Join-Path $PSScriptRoot 'desktop-build-info.ps1')
    Read-DesktopEmbeddedBuildInfo -Executable $main -ExpectedProfile release | Out-Null
    $mainEntry = $expected | Where-Object path -EQ 'audioprocess-prototype.exe'
    if ((Get-FileHash -LiteralPath $main -Algorithm SHA256).Hash.ToLowerInvariant() -cne $nsisMain.input_sha256) { throw 'Main image changed unexpectedly during bundling.' }
    $mainEntry.sha256 = $nsisMain.sha256
    $mainEntry.bytes = $nsisMain.bytes
    $expected | ConvertTo-Json -Depth 4 | Set-Content -LiteralPath (Join-Path $stage 'expected-payload.json') -Encoding utf8NoBOM
    $built = Join-Path $native "target/release/bundle/nsis/AudioGraph_${version}_x64-setup.exe"
    if (!(Test-Path -LiteralPath $built)) { throw 'Expected installer output was not generated.' }
    $generatedScript = Join-Path $native 'target/release/nsis/x64/installer.nsi'
    Copy-Item -LiteralPath $generatedScript -Destination (Join-Path $stage 'installer.nsi')
    if (Test-Path -LiteralPath $Destination) { throw 'Installer destination exists; choose another path. Nothing overwritten.' }
    [IO.File]::Copy($built,$Destination,$false)
    @{installer=$Destination;sha256=(Get-FileHash -LiteralPath $Destination).Hash.ToLowerInvariant();stage=$stage;bootstrapper=$bootstrapRecord;main_image=$nsisMain;tauri_cli=$cli.version;template_sha256=(Get-FileHash -LiteralPath (Join-Path $native 'installer/audio-graph.nsi')).Hash;generated_script_sha256=(Get-FileHash -LiteralPath $generatedScript).Hash;installed_or_launched=$false} |
        ConvertTo-Json -Depth 5 | Set-Content -LiteralPath ($Destination+'.build.json') -Encoding utf8NoBOM
    Write-Host "Installer built, not executed: $Destination"
    Write-Host "Verification staging: $stage"
} finally { Set-Location -LiteralPath $oldLocation.Path }
