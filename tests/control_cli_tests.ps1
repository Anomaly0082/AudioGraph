param([Parameter(Mandatory=$true)][string]$Executable, [Parameter(Mandatory=$true)][string]$TestRoot)
$ErrorActionPreference = 'Stop'
$testBase = [IO.Path]::GetFullPath((Join-Path $TestRoot ([Guid]::NewGuid().ToString('N'))))
$workspace = Join-Path $testBase 'workspace'
$outside = Join-Path $testBase 'outside'
New-Item -ItemType Directory -Path $workspace -Force | Out-Null
New-Item -ItemType Directory -Path $outside -Force | Out-Null
# Retain the unique test directory under build for diagnostics; no broad recursive deletion.
$startInfo = New-Object Diagnostics.ProcessStartInfo
$startInfo.FileName = $Executable
$startInfo.Arguments = '--workspace "' + $workspace + '"'
$startInfo.UseShellExecute = $false
$startInfo.CreateNoWindow = $true
$startInfo.RedirectStandardInput = $true
$startInfo.RedirectStandardOutput = $true
$startInfo.RedirectStandardError = $true
$utf8 = New-Object Text.UTF8Encoding($false)
$startInfo.StandardOutputEncoding = $utf8
$startInfo.StandardErrorEncoding = $utf8
$process = New-Object Diagnostics.Process
$process.StartInfo = $startInfo
if (-not $process.Start()) { throw 'Cannot start control-cli' }
$process.StandardInput.AutoFlush = $true
$errorsRead = $process.StandardError.ReadToEndAsync()
function Read-Response {
    $pending = $process.StandardOutput.ReadLineAsync()
    if (-not $pending.Wait(5000)) { throw 'Protocol response timeout' }
    if ($null -eq $pending.Result) { throw 'Unexpected EOF' }
    return ($pending.Result | ConvertFrom-Json)
}
function Send-Request($request) {
    $process.StandardInput.WriteLine(($request | ConvertTo-Json -Depth 20 -Compress))
    return (Read-Response)
}
try {
    $caps = Send-Request @{schema_version=1; id='discover'; op='capabilities'}
    if (-not $caps.success -or $caps.id -ne 'discover' -or $caps.data.policy.allow_devices) { throw 'Invalid capability response' }
    $process.StandardInput.WriteLine('{broken')
    $bad = Read-Response
    if ($bad.success) { throw 'Malformed request accepted' }
    $process.StandardInput.WriteLine((' ' * (4 * 1024 * 1024 + 1)))
    $bad = Read-Response
    if ($bad.success -or $bad.errors[0].code -ne 'invalid_request') { throw 'Oversized line accepted' }
    $graph = @{schema_version=1; nodes=@(@{id='source'; type='text_input'; parameters=@{text='IPC result'}});
        connections=@(); exports=@(@{name='message'; node='source'; port='text'})}
    $validation = Send-Request @{schema_version=1; id='validate'; op='graph.validate'; mode='offline'; graph=$graph}
    if (-not $validation.success -or $validation.data.device_access) { throw 'Validation failed' }
    $started = Send-Request @{schema_version=1; id='start'; op='tasks.start'; mode='offline'; graph=$graph}
    if (-not $started.success) { throw 'Task start failed' }
    $taskId = $started.data.task_id
    $terminal = $false
    for ($attempt=0; $attempt -lt 100; $attempt++) {
        $status = Send-Request @{schema_version=1; id='status'; op='tasks.status'; task_id=$taskId}
        if ($status.data.state -eq 'succeeded') { $terminal = $true; break }
        if ($status.data.state -eq 'failed') { throw 'Text task failed' }
        Start-Sleep -Milliseconds 5
    }
    if (-not $terminal) { throw 'Task did not complete' }
    $result = Send-Request @{schema_version=1; id='result'; op='tasks.result'; task_id=$taskId}
    if ($result.data.result.outputs.message.value -ne 'IPC result') { throw 'Wrong process task result' }
    $cancel = Send-Request @{schema_version=1; id='cancel'; op='tasks.cancel'; task_id=$taskId}
    if ($cancel.data.state -ne 'succeeded') { throw 'Late cancel changed terminal state' }
    $release = Send-Request @{schema_version=1; id='release'; op='tasks.release'; task_id=$taskId}
    if (-not $release.data.released) { throw 'Release failed' }

    # A Windows junction must not bypass the workspace boundary (no symlink privilege needed).
    New-Item -ItemType Junction -Path (Join-Path $workspace 'escape-link') -Target $outside | Out-Null
    $fileGraph = @{schema_version=1; nodes=@(
        @{id='source'; type='text_input'; parameters=@{text='must not write'}},
        @{id='sink'; type='text_output'; parameters=@{path='escape-link/escaped.txt'}});
        connections=@(@{from=@{node='source'; port='text'}; to=@{node='sink'; port='text'}})}
    $denied = Send-Request @{schema_version=1; id='escape'; op='tasks.start'; mode='offline'; graph=$fileGraph}
    if ($denied.success -or $denied.errors[0].code -ne 'path_not_allowed') { throw 'Junction escaped host workspace' }
    if (Test-Path -LiteralPath (Join-Path $outside 'escaped.txt')) { throw 'Denied request wrote outside workspace' }
    $devices = Send-Request @{schema_version=1; id='devices'; op='devices.list'}
    if ($devices.success -or $devices.errors[0].code -ne 'device_access_denied') { throw 'Device permission was not enforced' }
    $process.StandardInput.Close()
    if (-not $process.WaitForExit(5000)) { throw 'Control process did not shut down after EOF' }
    if ($process.ExitCode -ne 0) { throw ('Control process failed: ' + $errorsRead.Result) }
    Write-Output 'Persistent control process contract passed without device access.'
} finally {
    if (-not $process.HasExited) { $process.Kill(); $process.WaitForExit() }
    $process.Dispose()
}
