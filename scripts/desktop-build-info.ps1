#requires -Version 7.0
# Pure artifact inspection: this file never executes the inspected executable.
function Read-DesktopEmbeddedBuildInfo {
    [CmdletBinding()]
    param([Parameter(Mandatory)][string]$Executable)
    $begin = 'AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_BEGIN'
    $end = 'AUDIOPROCESS_DESKTOP_BUILD_INFO_V1_END'
    # Latin-1 is a lossless one-byte mapping, so arbitrary PE bytes cannot cause
    # decoding errors or shift/remove the ASCII sentinels we are inspecting.
    $image = [Text.Encoding]::GetEncoding(28591).GetString([IO.File]::ReadAllBytes($Executable))
    $start = $image.IndexOf($begin, [StringComparison]::Ordinal)
    $finish = $image.IndexOf($end, [StringComparison]::Ordinal)
    if ($start -lt 0 -or $finish -lt 0 -or
        $image.IndexOf($begin, $start + $begin.Length, [StringComparison]::Ordinal) -ge 0 -or
        $image.IndexOf($end, $finish + $end.Length, [StringComparison]::Ordinal) -ge 0) {
        throw 'Executable build metadata is missing or ambiguous; rebuild with the Tauri CLI. Sidecar markers and timestamps cannot establish its build mode.'
    }
    $payloadStart = $start + $begin.Length
    $length = $finish - $payloadStart
    if ($length -le 0 -or $length -gt 512) { throw 'Executable build metadata has an invalid length or sentinel order.' }
    $payload = $image.Substring($payloadStart, $length)
    $document = $null
    try {
        $document = [System.Text.Json.JsonDocument]::Parse($payload)
        $root = $document.RootElement
        if ($root.ValueKind -ne [System.Text.Json.JsonValueKind]::Object) { throw 'Build metadata must be an object.' }
        $names = [Collections.Generic.HashSet[string]]::new([StringComparer]::Ordinal)
        foreach ($property in $root.EnumerateObject()) {
            if (!$names.Add($property.Name) -or $property.Name -cnotin @('schema_version', 'profile', 'frontend_embedded')) {
                throw 'Build metadata contains duplicate or unknown fields.'
            }
        }
        if ($names.Count -ne 3) { throw 'Build metadata is incomplete.' }
        $schema = $root.GetProperty('schema_version')
        $buildProfileProperty = $root.GetProperty('profile')
        $embedded = $root.GetProperty('frontend_embedded')
        if ($schema.ValueKind -ne [System.Text.Json.JsonValueKind]::Number -or $schema.GetRawText() -cne '1' -or
            $buildProfileProperty.ValueKind -ne [System.Text.Json.JsonValueKind]::String -or
            $embedded.ValueKind -notin @([System.Text.Json.JsonValueKind]::True, [System.Text.Json.JsonValueKind]::False)) {
            throw 'Build metadata has an unsupported schema or invalid field types.'
        }
        if ($buildProfileProperty.GetString() -cne 'debug' -or !$embedded.GetBoolean()) {
            throw 'An embedded-frontend debug application is required. First run npm run tauri -- build --debug --no-bundle from apps/desktop; ordinary cargo build is not accepted.'
        }
        return [pscustomobject][ordered]@{ schema_version = 1; profile = 'debug'; frontend_embedded = $true }
    } catch {
        throw "Cannot verify executable build metadata: $($_.Exception.Message)"
    } finally {
        if ($null -ne $document) { $document.Dispose() }
    }
}
