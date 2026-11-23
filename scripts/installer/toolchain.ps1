Set-StrictMode -Version Latest

function Get-BlazerInstallerInputs {
    Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'inputs.json') | ConvertFrom-Json
}

function Get-VerifiedArchive {
    param([string]$Url, [string]$Sha256, [string]$Destination)

    if (Test-Path -LiteralPath $Destination -PathType Leaf) {
        if ((Get-FileHash -LiteralPath $Destination -Algorithm SHA256).Hash -ieq $Sha256) { return }
        throw "Untrusted existing build input: $Destination"
    }
    $parent = Split-Path -Parent $Destination
    [IO.Directory]::CreateDirectory($parent) | Out-Null
    $temporary = Join-Path $parent ([Guid]::NewGuid().ToString('N') + '.download')
    try {
        Invoke-WebRequest -Uri $Url -OutFile $temporary -TimeoutSec 120
        if ((Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash -ine $Sha256) {
            throw "Build input digest mismatch: $Url"
        }
        [IO.File]::Move($temporary, $Destination)
    } finally { if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) } }
}

function Get-BlazerNsisCompiler {
    param([string]$Compiler)

    $inputs = Get-BlazerInstallerInputs
    if (-not $Compiler) {
        $root = if ($env:RUNNER_TEMP) { Join-Path $env:RUNNER_TEMP 'blazer-tools/nsis-3.11' }
            else { 'C:\Development\NSIS\3.11' }
        $Compiler = Join-Path $root 'makensis.exe'
        if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
            $archive = Join-Path (Split-Path -Parent $root) 'nsis-3.11.zip'
            Get-VerifiedArchive $inputs.nsis.url $inputs.nsis.sha256 $archive
            $zip = [IO.Compression.ZipFile]::OpenRead($archive)
            try {
                foreach ($entry in $zip.Entries) {
                    if (-not $entry.FullName.StartsWith('nsis-3.11/')) { throw 'Unexpected NSIS archive entry.' }
                    $relative = $entry.FullName.Substring('nsis-3.11/'.Length)
                    if (-not $relative) { continue }
                    $path = [IO.Path]::GetFullPath((Join-Path $root $relative))
                    if (-not $path.StartsWith($root.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)) {
                        throw 'NSIS archive entry escaped the tool directory.'
                    }
                    if ($entry.Name) {
                        [IO.Directory]::CreateDirectory((Split-Path -Parent $path)) | Out-Null
                        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $path, $false)
                    } else { [IO.Directory]::CreateDirectory($path) | Out-Null }
                }
            } finally { $zip.Dispose() }
        }
    }
    $version = & $Compiler /VERSION
    if ($LASTEXITCODE -ne 0 -or $version -cne ('v' + $inputs.nsis.version)) { throw 'NSIS 3.11 is required.' }
    [IO.Path]::GetFullPath($Compiler)
}

function Get-BlazerWintunDriver {
    param([string]$Driver, [string]$CacheDirectory)

    $wintunInput = (Get-BlazerInstallerInputs).wintun
    if (-not $Driver) {
        $Driver = Join-Path $CacheDirectory 'wintun-0.14.1/wintun.dll'
        if (-not (Test-Path -LiteralPath $Driver -PathType Leaf)) {
            $archive = Join-Path $CacheDirectory 'wintun-0.14.1.zip'
            Get-VerifiedArchive $wintunInput.url $wintunInput.archive_sha256 $archive
            $zip = [IO.Compression.ZipFile]::OpenRead($archive)
            try {
                $entry = $zip.GetEntry($wintunInput.entry)
                if (-not $entry) { throw 'The approved Wintun archive has no AMD64 driver.' }
                [IO.Directory]::CreateDirectory((Split-Path -Parent $Driver)) | Out-Null
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $Driver, $false)
            } finally { $zip.Dispose() }
        }
    }
    if ((Get-FileHash -LiteralPath $Driver -Algorithm SHA256).Hash -ine $wintunInput.sha256) {
        throw 'Wintun DLL does not match the approved AMD64 digest.'
    }
    [IO.Path]::GetFullPath($Driver)
}
