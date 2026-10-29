# Tauri CLI 2.11.4 temporarily changes this three-byte hint while bundling, then
# restores the source image. Predict only that documented/pinned byte change.
function Get-NsisExpectedImage {
    param([Parameter(Mandatory)][string]$Executable)
    $bytes = [IO.File]::ReadAllBytes($Executable)
    $text = [Text.Encoding]::GetEncoding(28591).GetString($bytes)
    $marker = '__TAURI_BUNDLE_TYPE_VAR_'
    $start = $text.IndexOf($marker,[StringComparison]::Ordinal)
    if ($start -lt 0 -or $text.IndexOf($marker,$start+$marker.Length,[StringComparison]::Ordinal) -ge 0) { throw 'Missing or ambiguous Tauri bundle-kind marker.' }
    $offset = $start + $marker.Length
    if ($offset + 3 -gt $bytes.Length -or $text.Substring($offset,3) -cnotin @('UNK','NSS')) { throw 'Unexpected Tauri bundle kind; review the pinned bundler.' }
    $inputHash = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant()
    [Text.Encoding]::ASCII.GetBytes('NSS').CopyTo($bytes,$offset)
    return @{input_sha256=$inputHash;sha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($bytes)).ToLowerInvariant();bytes=$bytes.Length;bundle_marker='NSS'}
}
