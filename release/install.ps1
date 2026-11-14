param([Parameter(Mandatory=$true)][ValidatePattern('^\d+\.\d+\.\d+$')][string]$Version, [string]$Prefix = "$env:LOCALAPPDATA\WebTest\bin")
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Net.Http
function Get-Bounded([string]$Url, [string]$Destination, [long]$Limit) {
    $client = [Net.Http.HttpClient]::new()
    $deadline = [Threading.CancellationTokenSource]::new([TimeSpan]::FromSeconds(120))
    $response = $null; $inputStream = $null; $outputStream = $null
    try {
        $response = $client.GetAsync($Url, [Net.Http.HttpCompletionOption]::ResponseHeadersRead, $deadline.Token).GetAwaiter().GetResult()
        $response.EnsureSuccessStatusCode() | Out-Null
        if ($response.RequestMessage.RequestUri.Scheme -ne 'https' -or $response.Content.Headers.ContentLength -gt $Limit) { throw 'Insecure or oversized release response' }
        $inputStream = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        $outputStream = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew)
        $buffer = New-Object byte[] 65536
        [long]$total = 0
        while (($count = $inputStream.ReadAsync($buffer, 0, $buffer.Length, $deadline.Token).GetAwaiter().GetResult()) -gt 0) {
            $total += $count
            if ($total -gt $Limit) { throw 'Release byte limit exceeded' }
            $outputStream.Write($buffer, 0, $count)
        }
    } finally {
        if ($outputStream) { $outputStream.Dispose() }; if ($inputStream) { $inputStream.Dispose() }
        if ($response) { $response.Dispose() }; $deadline.Dispose(); $client.Dispose()
    }
}
if (![Environment]::Is64BitOperatingSystem -or $env:PROCESSOR_ARCHITECTURE -ne 'AMD64') { throw 'Only Windows x86_64 is supported; build from source.' }
Get-Command cosign -ErrorAction Stop | Out-Null
$temp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory $temp | Out-Null
try {
    $base = "https://github.com/williamcotton/webtest/releases/download/v$Version"
    Get-Bounded "$base/manifest.json" "$temp/manifest.json" 1048576
    Get-Bounded "$base/manifest.sigstore.json" "$temp/manifest.sigstore.json" 1048576
    if ((Get-Item "$temp/manifest.json").Length -gt 1048576 -or (Get-Item "$temp/manifest.sigstore.json").Length -gt 1048576) { throw 'Manifest limit exceeded' }
    & cosign verify-blob --bundle "$temp/manifest.sigstore.json" --certificate-oidc-issuer https://token.actions.githubusercontent.com --certificate-identity "https://github.com/williamcotton/webtest/.github/workflows/release.yml@refs/tags/v$Version" "$temp/manifest.json"
    if ($LASTEXITCODE -ne 0) { throw 'Signature verification failed' }
    $manifest = Get-Content "$temp/manifest.json" -Raw | ConvertFrom-Json
    if ($manifest.schema -ne 1 -or $manifest.version -ne $Version) { throw 'Unsupported manifest' }
    $artifacts = @($manifest.artifacts | Where-Object { $_.kind -eq 'binary' -and $_.target -eq 'x86_64-pc-windows-msvc' })
    if ($artifacts.Count -ne 1) { throw 'Missing or ambiguous platform binary' }
    $artifact = $artifacts[0]
    if ($artifact.name -notmatch '^[a-zA-Z0-9][a-zA-Z0-9._-]{0,180}$' -or $artifact.bytes -le 0 -or $artifact.bytes -gt 268435456) { throw 'Unsafe artifact' }
    Get-Bounded "$base/$($artifact.name)" "$temp/webtest.exe" $artifact.bytes
    if ((Get-Item "$temp/webtest.exe").Length -ne $artifact.bytes -or (Get-FileHash "$temp/webtest.exe" -Algorithm SHA256).Hash.ToLower() -ne $artifact.sha256) { throw 'Checksum mismatch' }
    New-Item -ItemType Directory -Force $Prefix | Out-Null
    $staged = Join-Path $Prefix ('.webtest-' + [Guid]::NewGuid().ToString('N') + '.exe')
    Copy-Item "$temp/webtest.exe" $staged
    Move-Item -Force $staged (Join-Path $Prefix 'webtest.exe')
    Write-Output "Installed WebTest $Version at $Prefix. Shell PATH and Chrome were not changed."
} finally { Remove-Item -Recurse -Force $temp }
