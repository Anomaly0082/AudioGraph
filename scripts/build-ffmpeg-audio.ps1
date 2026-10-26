param(
    [string]$MsysRoot = '',
    [string]$CacheDirectory = '',
    [string]$BuildDirectory = '',
    [string]$CmpExecutable = '',
    [ValidateRange(1,4)][int]$Jobs = 4,
    [switch]$Offline
)
$ErrorActionPreference = 'Stop'
$repo = Split-Path $PSScriptRoot -Parent
if (!$MsysRoot) {
    $gccCommand = Get-Command gcc.exe -ErrorAction SilentlyContinue
    $candidates = @()
    if ($gccCommand) { $candidates += Split-Path (Split-Path (Split-Path $gccCommand.Source -Parent) -Parent) -Parent }
    $candidates += @('C:/msys64','C:/AAATestTemmmmmm/MSYS2')
    $MsysRoot = $candidates | Where-Object { Test-Path (Join-Path $_ 'usr/bin/bash.exe') } | Select-Object -First 1
    if (!$MsysRoot) { throw 'Pass -MsysRoot pointing to an existing MSYS2 with mingw64 GCC.' }
}
if (!$CacheDirectory) { $CacheDirectory = Join-Path $repo 'build/ffmpeg-cache' }
if (!$BuildDirectory) { $BuildDirectory = Join-Path $repo 'build/ffmpeg-audio' }
$CacheDirectory = [IO.Path]::GetFullPath($CacheDirectory)
$BuildDirectory = [IO.Path]::GetFullPath($BuildDirectory)
$lockPath = Join-Path $PSScriptRoot 'ffmpeg-audio.lock.json'
$lock = Get-Content $lockPath -Raw | ConvertFrom-Json
$bash = Join-Path $MsysRoot 'usr/bin/bash.exe'
if (!(Test-Path $bash)) { throw "MSYS2 bash missing: $bash" }
if (!$CmpExecutable) {
    $CmpExecutable = @((Join-Path $MsysRoot 'usr/bin/cmp.exe'),'C:/Program Files/Git/usr/bin/cmp.exe') |
        Where-Object { Test-Path $_ } | Select-Object -First 1
}
if (!$CmpExecutable -or !(Test-Path $CmpExecutable)) { throw 'GNU cmp is required; pass -CmpExecutable from an existing MSYS2/Git installation.' }
New-Item $CacheDirectory,$BuildDirectory -ItemType Directory -Force | Out-Null
foreach ($inputSource in @($lock.ffmpeg_source,$lock.lame_source,$lock.make_tool)) {
    $archive = Join-Path $CacheDirectory $inputSource.name
    if (!(Test-Path $archive)) {
        if ($Offline -or !$inputSource.url) { throw "Missing cached input: $archive" }
        Invoke-WebRequest $inputSource.url -OutFile $archive
    }
    if ((Get-FileHash $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $inputSource.sha256) {
        throw "Input SHA256 mismatch: $archive"
    }
}
function PosixPath([string]$path) {
    $full = [IO.Path]::GetFullPath($path).Replace('\','/')
    if ($full -notmatch '^([A-Za-z]):/(.*)$') { throw "Expected drive-absolute path: $path" }
    return '/' + $matches[1].ToLowerInvariant() + '/' + $matches[2]
}
$metadata = Join-Path $BuildDirectory 'metadata'
New-Item $metadata -ItemType Directory -Force | Out-Null
Get-ChildItem (Join-Path $MsysRoot 'var/lib/pacman/local') -Directory | Where-Object {
    $_.Name -match '^(msys2-runtime|bash-|coreutils-|grep-|sed-|gawk-|perl-|mingw-w64-x86_64-(gcc|gcc-libs|binutils|headers|crt|libwinpthread))'
} | Select-Object -ExpandProperty Name | Set-Content (Join-Path $metadata 'toolchain-packages.txt') -Encoding utf8
$toolFiles = @('mingw64/bin/gcc.exe','mingw64/bin/ld.exe','mingw64/bin/as.exe','mingw64/bin/strip.exe','mingw64/bin/ar.exe','mingw64/bin/ranlib.exe',
    'mingw64/bin/objdump.exe','mingw64/bin/pkg-config.exe','usr/bin/bash.exe','usr/bin/sed.exe') | ForEach-Object {
    $toolPath = Join-Path $MsysRoot $_
    @{path=$_;sha256=(Get-FileHash $toolPath).Hash.ToLowerInvariant()}
}
foreach ($programName in @('cc1','collect2')) {
    $programPath = & (Join-Path $MsysRoot 'mingw64/bin/gcc.exe') "-print-prog-name=$programName"
    $resolvedProgramPath = [IO.Path]::GetFullPath($programPath)
    $toolFiles += @{path=[IO.Path]::GetRelativePath($MsysRoot,$resolvedProgramPath);sha256=(Get-FileHash $resolvedProgramPath).Hash.ToLowerInvariant()}
}
$toolFiles | ConvertTo-Json | Set-Content (Join-Path $metadata 'tool-executable-sha256.json') -Encoding utf8
& (Join-Path $MsysRoot 'mingw64/bin/ld.exe') --version | Set-Content (Join-Path $metadata 'linker-identification.txt') -Encoding utf8
$lockHash = (Get-FileHash $lockPath).Hash
$stamp = Join-Path $BuildDirectory 'input-lock-sha256.txt'
if ((Test-Path $stamp) -and (Get-Content $stamp -Raw).Trim() -ne $lockHash) {
    throw 'Build directory belongs to another source/configuration lock. Choose a new BuildDirectory.'
}
$lockHash | Set-Content $stamp
$ffmpegSource = Join-Path $BuildDirectory ('src/FFmpeg-' + $lock.ffmpeg_source.commit)
if (!(Test-Path (Join-Path $ffmpegSource 'configure'))) {
    New-Item (Join-Path $BuildDirectory 'src') -ItemType Directory -Force | Out-Null
    Expand-Archive (Join-Path $CacheDirectory $lock.ffmpeg_source.name) (Join-Path $BuildDirectory 'src')
}
$env:AUDIO_BUILD_ROOT = PosixPath $BuildDirectory
$env:AUDIO_CACHE_ROOT = PosixPath $CacheDirectory
$env:AUDIO_SCRIPT_ROOT = PosixPath $PSScriptRoot
$env:AUDIO_CMP_DIRECTORY = PosixPath (Split-Path $CmpExecutable -Parent)
$env:AUDIO_LAME_ARCHIVE = $lock.lame_source.name
$env:AUDIO_MAKE_ARCHIVE = $lock.make_tool.name
$env:AUDIO_FFMPEG_COMMIT = $lock.ffmpeg_source.commit
$env:AUDIO_JOBS = "$Jobs"
$env:AUDIO_DECODERS = $lock.decoders -join ','
($lock.decoders -join ',') | Set-Content (Join-Path $metadata 'decoder-configure-ids.txt') -Encoding ascii
$env:AUDIO_ENCODERS = $lock.encoder_names -join ','
$env:AUDIO_DEMUXERS = $lock.demuxer_names -join ','
$env:AUDIO_MUXERS = $lock.muxer_names -join ','
$env:AUDIO_FILTERS = $lock.filter_names -join ','
$driverSnapshot = Join-Path $BuildDirectory 'build-driver.sh'
Copy-Item (Join-Path $PSScriptRoot 'build-ffmpeg-audio.sh') $driverSnapshot
& $bash --noprofile --norc (PosixPath $driverSnapshot)
if ($LASTEXITCODE) { throw "Audio FFmpeg build failed ($LASTEXITCODE); see $BuildDirectory logs" }
$toolFiles += @{path='project-local tools/usr/bin/make.exe';sha256=(Get-FileHash (Join-Path $BuildDirectory 'tools/usr/bin/make.exe')).Hash.ToLowerInvariant()}
$toolFiles += @{path=$CmpExecutable;sha256=(Get-FileHash $CmpExecutable).Hash.ToLowerInvariant()}
$toolFiles | ConvertTo-Json | Set-Content (Join-Path $metadata 'tool-executable-sha256.json') -Encoding utf8
Copy-Item $lockPath $metadata
$actualDecoders = @(Get-Content (Join-Path $metadata 'decoders.txt') | ForEach-Object {
    if ($_ -match '^ ([VAS])[\w.]{5}\s+([^\s=]+)') {
        if ($matches[1] -ne 'A') { throw "Non-audio decoder enabled: $($matches[2])" }
        $matches[2]
    }
})
$missingDecoders = @($lock.baseline_builtin_audio_cli_names | Where-Object { $_ -notin $actualDecoders })
@{baseline_builtin_count=$lock.baseline_builtin_audio_cli_names.Count;actual_count=$actualDecoders.Count;missing=$missingDecoders} |
    ConvertTo-Json -Depth 5 | Set-Content (Join-Path $metadata 'decoder-coverage.json') -Encoding utf8
if ($missingDecoders.Count) { throw "Audio decoder coverage decreased: $($missingDecoders -join ', ')" }
$flags = @(Get-Content (Join-Path $metadata 'configure-flags.txt'))
$flags | ConvertTo-Json | Set-Content (Join-Path $metadata 'configure-flags.json') -Encoding utf8
$runtimeRecords = @()
$objdump = Join-Path $MsysRoot 'mingw64/bin/objdump.exe'
$needsWinpthread = $false
Get-ChildItem (Join-Path $BuildDirectory 'stage/bin') -File | Where-Object Extension -in '.exe','.dll' | ForEach-Object {
    if (@(& $objdump -p $_.FullName) -match 'DLL Name:\s*libwinpthread-1\.dll') { $needsWinpthread = $true }
    if ($LASTEXITCODE) { throw "PE dependency inspection failed: $($_.Name)" }
}
if ($needsWinpthread) {
    $runtimeSource = Join-Path $MsysRoot 'mingw64/bin/libwinpthread-1.dll'
    if (!(Test-Path $runtimeSource)) { throw "Required runtime missing: $runtimeSource" }
    Copy-Item $runtimeSource (Join-Path $BuildDirectory 'stage/bin/libwinpthread-1.dll') -Force
    $runtimeRecords += @{
        name='libwinpthread-1.dll'; source='mingw64/bin/libwinpthread-1.dll'
        sha256=(Get-FileHash $runtimeSource).Hash.ToLowerInvariant()
        package=@(Get-ChildItem (Join-Path $MsysRoot 'var/lib/pacman/local') -Directory -Filter 'mingw-w64-x86_64-libwinpthread-*' | Select-Object -ExpandProperty Name)
        license_source='mingw64/share/licenses/winpthreads/COPYING'
    }
}
$runtimeRecords | ConvertTo-Json -Depth 5 -AsArray | Set-Content (Join-Path $metadata 'runtime-redistributables.json') -Encoding utf8
$binaryFiles = @(Get-ChildItem (Join-Path $BuildDirectory 'stage/bin') -File | Where-Object Extension -in '.exe','.dll')
$imports = @{}
foreach ($binary in $binaryFiles) {
    $importNames = @(& $objdump -p $binary.FullName | ForEach-Object {
        if ($_ -match 'DLL Name:\s*(\S+)') { $matches[1] }
    })
    if ($LASTEXITCODE) { throw "PE dependency inspection failed: $($binary.Name)" }
    if ($importNames -match 'libstdc\+\+|msys-2.0|libgcc_s') { throw "Unexpected toolchain runtime dependency: $($binary.Name)" }
    $imports[$binary.Name] = $importNames
}
$imports | ConvertTo-Json -Depth 5 | Set-Content (Join-Path $metadata 'pe-imports.json') -Encoding utf8
$importText = foreach ($binaryName in ($imports.Keys | Sort-Object)) {
    "FILE: $binaryName"
    $imports[$binaryName] | ForEach-Object { "  DLL Name: $_" }
    ''
}
$importText | Set-Content (Join-Path $metadata 'pe-imports.txt') -Encoding utf8
$manifest = [ordered]@{
    schema_version = 1
    profile = $lock.profile
    source_inputs = @($lock.ffmpeg_source,$lock.lame_source)
    build_tool_input = $lock.make_tool
    lock_sha256 = $lockHash.ToLowerInvariant()
    link_policy = $lock.link_policy
    source_patches = $lock.source_patches
    runtime_redistributables = $runtimeRecords
    configure_flags = $flags
    binaries = @($binaryFiles | ForEach-Object { @{name=$_.Name; size=$_.Length; sha256=(Get-FileHash $_.FullName).Hash.ToLowerInvariant()} })
}
$manifest | ConvertTo-Json -Depth 10 | Set-Content (Join-Path $metadata 'build-manifest.json') -Encoding utf8
Write-Host "Audio FFmpeg stage: $BuildDirectory/stage"
