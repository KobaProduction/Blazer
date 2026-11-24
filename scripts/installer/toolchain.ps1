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
        $response = Invoke-WebRequest -Uri $Url -OutFile $temporary -TimeoutSec 120 -UserAgent 'Blazer-Build/1.0' -PassThru
        $actual = (Get-FileHash -LiteralPath $temporary -Algorithm SHA256).Hash
        if ($actual -ine $Sha256) {
            $size = ([IO.FileInfo]$temporary).Length
            $type = [string]$response.Headers['Content-Type']
            throw "Build input digest mismatch: $Url (type=$type, bytes=$size, actual=$actual, expected=$Sha256)"
        }
        [IO.File]::Move($temporary, $Destination)
    } finally { if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) } }
}

function Get-BlazerNsisCompiler {
    param([string]$Compiler)

    $inputs = Get-BlazerInstallerInputs
    $folder = 'nsis-' + $inputs.nsis.version
    if (-not $Compiler) {
        $root = if ($env:RUNNER_TEMP) { Join-Path $env:RUNNER_TEMP ('blazer-tools/' + $folder) }
            else { Join-Path 'C:\Development\NSIS' $inputs.nsis.version }
        $Compiler = Join-Path $root 'makensis.exe'
        if (-not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
            $archive = Join-Path (Split-Path -Parent $root) ($folder + '.zip')
            Get-VerifiedArchive $inputs.nsis.url $inputs.nsis.sha256 $archive
            $zip = [IO.Compression.ZipFile]::OpenRead($archive)
            try {
                foreach ($entry in $zip.Entries) {
                    if (-not $entry.FullName.StartsWith($folder + '/')) { throw 'Unexpected NSIS archive entry.' }
                    $relative = $entry.FullName.Substring(($folder + '/').Length)
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
    if ($LASTEXITCODE -ne 0 -or $version -cne ('v' + $inputs.nsis.version)) { throw ('NSIS ' + $inputs.nsis.version + ' is required.') }
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
