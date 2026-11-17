#Requires -Version 7.0
param(
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',
    [string]$OutputDirectory,
    [string]$BlazeCorePath,
    [string]$BlazeCoreLicensePath,
    [string]$WintunLicensePath,
    [switch]$ReleaseArchive,
    [switch]$PreviewArchive
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'release-version.ps1')
$wintunLicenseSha256 = '183ADAC21E7D96C508C8FD34D394B7B6708BC81564AD1BAD61AB66143A008CD2'
if (($ReleaseArchive -or $PreviewArchive) -and $Configuration -ne 'Release') { throw 'Release archives require -Configuration Release.' }
if ($ReleaseArchive -and $PreviewArchive) { throw 'Select one archive channel.' }

function Assert-PackageDirectory {
    param([string]$Directory)

    if (Test-Path -LiteralPath $Directory) {
        $item = Get-Item -LiteralPath $Directory -Force
        if (-not $item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "The package path must be a regular directory: '$Directory'."
        }
    }
}

function Assert-PackageFileReplaceable {
    param([string]$Path)

    if (-not (Test-Path -LiteralPath $Path)) { return }
    $item = Get-Item -LiteralPath $Path -Force
    if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "The package file must be a regular file: '$Path'."
    }
    try {
        [IO.File]::Open($Path, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None).Dispose()
    } catch {
        throw "Cannot replace '$Path'. Close Blazer or any process using the package, then retry. $($_.Exception.Message)"
    }
}

function Publish-Package {
    param([string]$Name, [string]$Directory, [hashtable[]]$Files)

    Assert-PackageDirectory $Directory
    [IO.Directory]::CreateDirectory($output) | Out-Null
    $artifacts = foreach ($file in $Files) {
        $destination = Join-Path $Directory $file.Name
        $parent = Split-Path -Parent $destination
        Assert-PackageDirectory $parent
        $temporaryName = $file['TemporaryName'] ?? $file.Name
        [pscustomobject]@{
            Source = $file.Source
            ExpectedSha256 = $file['ExpectedSha256']
            Parent = $parent
            Destination = $destination
            Temporary = Join-Path $output ".blazer-$Name-$temporaryName.tmp"
        }
    }
    try {
        $staged = foreach ($artifact in $artifacts) {
            [IO.File]::Delete($artifact.Temporary)
            if ($artifact.ExpectedSha256 -and
                (Test-Path -LiteralPath $artifact.Destination -PathType Leaf) -and
                -not ((Get-Item -LiteralPath $artifact.Destination -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) -and
                (Get-FileHash -Algorithm SHA256 -LiteralPath $artifact.Destination).Hash -eq $artifact.ExpectedSha256) {
                continue
            }
            Assert-PackageFileReplaceable $artifact.Destination
            [IO.File]::Copy($artifact.Source, $artifact.Temporary)
            if ($artifact.ExpectedSha256 -and
                (Get-FileHash -Algorithm SHA256 -LiteralPath $artifact.Temporary).Hash -ne $artifact.ExpectedSha256) {
                throw "SHA-256 does not match the approved digest for '$($artifact.Destination)'."
            }
            $artifact
        }
        foreach ($artifact in $staged) {
            [IO.Directory]::CreateDirectory($artifact.Parent) | Out-Null
            [IO.File]::Move($artifact.Temporary, $artifact.Destination, $true)
        }
    } finally {
        foreach ($artifact in $artifacts) {
            [IO.File]::Delete($artifact.Temporary)
        }
    }
}

function Get-EngineNoticeFiles {
    param([string]$EngineLicense)

    $source = Split-Path -Parent $EngineLicense
    $notice = Join-Path $source 'THIRD_PARTY_NOTICES.md'
    if (Test-Path -LiteralPath $notice -PathType Leaf) {
        @{ Name = 'THIRD_PARTY_NOTICES.md'; Source = $notice }
    }

    $licenses = Join-Path $source 'licenses'
    if (-not (Test-Path -LiteralPath $licenses -PathType Container)) { return }
    Assert-PackageDirectory $licenses
    $index = 0
    foreach ($license in (Get-ChildItem -LiteralPath $licenses -Force | Sort-Object Name)) {
        if ($license.PSIsContainer -or ($license.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw "Expected a regular file in the BlazeCore license bundle: '$($license.FullName)'."
        }
        $index++
        @{
            Name = "licenses/$($license.Name)"
            Source = $license.FullName
            TemporaryName = "license-bundle-$index"
        }
    }
}

function Get-ApprovedSha256 {
    param([string]$Source, [string]$Constant)

    $content = Get-Content -Raw -LiteralPath $Source
    $digest = [regex]::Match($content, ('pub const ' + $Constant + ':\s*&str\s*=\s*"([A-Fa-f0-9]{64})"'))
    if (-not $digest.Success) { throw "Could not read $Constant from '$Source'." }
    $digest.Groups[1].Value
}

function Resolve-WintunLicense {
    if ($WintunLicensePath) {
        $candidates = @([IO.Path]::GetFullPath($WintunLicensePath, $workingDirectory))
    } else {
        $cargoHome = if ($env:CARGO_HOME) {
            [IO.Path]::GetFullPath($env:CARGO_HOME, $repo)
        } else {
            Join-Path $env:USERPROFILE '.cargo'
        }
        $registrySource = Join-Path $cargoHome 'registry/src'
        $candidates = @(if (Test-Path -LiteralPath $registrySource -PathType Container) {
            Get-ChildItem -LiteralPath $registrySource -Directory | ForEach-Object {
                Join-Path $_.FullName 'wintun-bindings-0.7.40/wintun/LICENSE.txt'
            }
        })
    }
    foreach ($candidate in $candidates) {
        if ((Test-Path -LiteralPath $candidate -PathType Leaf) -and
            (Get-FileHash -Algorithm SHA256 -LiteralPath $candidate).Hash -eq $wintunLicenseSha256) {
            return $candidate
        }
    }
    throw 'Approved Wintun 0.14.1 prebuilt-binaries license is missing or has changed. Supply the official LICENSE.txt with -WintunLicensePath.'
}

function Build-Package {
    $previousVersion = $env:BLAZER_RELEASE_VERSION
    $previousUserAgent = $env:BLAZER_RELEASE_USER_AGENT
    $previousPreview = $env:BLAZER_PREVIEW_BUILD
    try {
        $env:BLAZER_RELEASE_VERSION = $releaseVersion
        $env:BLAZER_RELEASE_USER_AGENT = if ($releaseVersion) { "Blazer/$releaseVersion" } else { $null }
        $env:BLAZER_PREVIEW_BUILD = if ($PreviewArchive) { '1' } else { $null }
        & cargo build --locked --target $target --target-dir $targetDirectory -p blazer --bin blazer @profileArguments
        if ($LASTEXITCODE -ne 0) { throw "Cargo build failed for Blazer (exit $LASTEXITCODE)." }
    } finally {
        $env:BLAZER_RELEASE_VERSION = $previousVersion
        $env:BLAZER_RELEASE_USER_AGENT = $previousUserAgent
        $env:BLAZER_PREVIEW_BUILD = $previousPreview
    }

    $driver = Join-Path $buildDirectory 'wintun.dll'
    if (-not (Test-Path -LiteralPath $driver -PathType Leaf)) {
        throw "The client build did not provide a Wintun DLL at '$driver'."
    }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $driver).Hash -ne $driverSha256) {
        throw "Wintun SHA-256 does not match APPROVED_WINTUN_SHA256: '$driver'."
    }
    $driverLicense = Resolve-WintunLicense

    $directory = Join-Path $output 'client'
    $files = @(
        @{ Name = 'LICENSE'; Source = Join-Path $repo 'LICENSE' }
        @{ Name = 'LICENSE-NOTO-EMOJI'; Source = Join-Path $repo 'src/presentation/client-ui/assets/LICENSE-NOTO-EMOJI.txt' }
        @{ Name = 'LICENSE-BLAZECORE'; Source = $BlazeCoreLicensePath }
        @{ Name = 'blazecore.exe'; Source = $BlazeCorePath; ExpectedSha256 = $engineSha256 }
        @{ Name = 'wintun.dll'; Source = $driver; ExpectedSha256 = $driverSha256 }
        @{ Name = 'LICENSE-WINTUN'; Source = $driverLicense; ExpectedSha256 = $wintunLicenseSha256 }
    )
    $files += @(Get-EngineNoticeFiles $BlazeCoreLicensePath)
    $executable = Join-Path $directory 'blazer.exe'
    Assert-PackageDirectory $directory
    Assert-PackageFileReplaceable $executable
    $app = @{ Name = 'blazer.exe'; Source = Join-Path $buildDirectory 'blazer.exe' }
    # For an existing package, replace the executable first so a late launch blocks publication.
    # A fresh package becomes launchable only after its companion files are installed.
    $files = if ([IO.File]::Exists($executable)) { @($app) + $files } else { $files + @($app) }
    $marker = $null
    try {
        if ($ReleaseArchive -or $PreviewArchive) {
            Assert-PackageDirectory $output
            [IO.Directory]::CreateDirectory($output) | Out-Null
            $marker = Join-Path $output ".blazer-package-$([Guid]::NewGuid().ToString('N')).tmp"
            $versionInfo = (Get-Item -LiteralPath $app.Source).VersionInfo.ProductVersion
            if ($versionInfo -cne $releaseVersion) { throw 'The Windows executable version does not match the Git tag.' }
            $channel = if ($PreviewArchive) { 'preview' } else { 'stable' }
            $manifest = @{ version = $releaseVersion; target = 'windows-x86_64'; channel = $channel } | ConvertTo-Json -Compress
            [IO.File]::WriteAllText($marker, $manifest)
            $files += @{ Name = 'blazer-package.json'; Source = $marker }
        }
        Publish-Package 'client' $directory $files
        Write-Output $executable
        if ($ReleaseArchive -or $PreviewArchive) { Write-ReleaseArchive $directory $files }
    } finally {
        if ($marker) { [IO.File]::Delete($marker) }
    }
}

function Write-ReleaseArchive {
    param([string]$Directory, [hashtable[]]$Files)

    $version = (Get-Item -LiteralPath (Join-Path $Directory 'blazer.exe')).VersionInfo.ProductVersion
    if ($version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$') {
        throw "Invalid X.Y version in the Blazer executable: '$version'."
    }
    $name = if ($PreviewArchive) { 'Blazer-preview-windows-x86_64.zip' } else { "Blazer-$version-windows-x86_64.zip" }
    $destination = Join-Path $output $name
    Assert-PackageFileReplaceable $destination
    $temporary = Join-Path $output ".blazer-release-$([Guid]::NewGuid().ToString('N')).tmp"
    try {
        $archive = [IO.Compression.ZipFile]::Open($temporary, [IO.Compression.ZipArchiveMode]::Create)
        try {
            foreach ($file in $Files) {
                [void][IO.Compression.ZipFileExtensions]::CreateEntryFromFile(
                    $archive, (Join-Path $Directory $file.Name), $file.Name,
                    [IO.Compression.CompressionLevel]::Optimal
                )
            }
        } finally {
            $archive.Dispose()
        }
        [IO.File]::Move($temporary, $destination, $true)
        Get-FileHash -Algorithm SHA256 -LiteralPath $destination
    } finally {
        [IO.File]::Delete($temporary)
    }
}

$repo = Split-Path -Parent $PSScriptRoot
$releaseVersion = if ($ReleaseArchive) { Get-ReleaseVersion $repo }
    elseif ($PreviewArchive) { Get-WorkspaceAppVersion $repo 'blazer' }
    else { $null }
$target = 'x86_64-pc-windows-msvc'
$targetDirectory = if ($env:CARGO_TARGET_DIR) { [IO.Path]::GetFullPath($env:CARGO_TARGET_DIR, $repo) }
    else { Join-Path $repo 'target' }
$targetPlatformDirectory = Join-Path $targetDirectory $target
$buildDirectory = Join-Path $targetPlatformDirectory $Configuration.ToLowerInvariant()
$profileArguments = @('--features', 'demo')
if ($Configuration -eq 'Release') { $profileArguments = @('--release') }
$workingDirectory = (Get-Location).ProviderPath
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'dist/windows' }
$output = [IO.Path]::GetFullPath($OutputDirectory, $workingDirectory)

function Resolve-EnginePackage {
    $neighbor = Split-Path -Parent $repo
    $engineSource = Join-Path $repo 'src/infrastructure/client_engine'
    $engineSha256 = Get-ApprovedSha256 (Join-Path $engineSource 'mod.rs') 'APPROVED_ENGINE_SHA256'
    $driverSha256 = Get-ApprovedSha256 (Join-Path $engineSource 'windows_tunnel.rs') 'APPROVED_WINTUN_SHA256'
    if (-not $BlazeCorePath) {
        $candidates = @(
            (Join-Path $neighbor "BlazeCore/downstream/artifacts/$target/release/blazecore.exe")
            (Join-Path $neighbor "BlazeCore/target/$target/debug/blazecore.exe")
            (Join-Path $targetPlatformDirectory 'release/blazecore.exe')
        )
        $BlazeCorePath = $candidates | Where-Object {
            (Test-Path -LiteralPath $_ -PathType Leaf) -and
                (Get-FileHash -Algorithm SHA256 -LiteralPath $_).Hash -eq $engineSha256
        } | Select-Object -First 1
        if (-not $BlazeCorePath) {
            throw 'No SHA-256-pinned BlazeCore executable was found. Supply it with -BlazeCorePath.'
        }
    }
    $BlazeCorePath = [IO.Path]::GetFullPath($BlazeCorePath, $workingDirectory)
    if (-not (Test-Path -LiteralPath $BlazeCorePath -PathType Leaf)) {
        throw "Approved BlazeCore executable is missing at '$BlazeCorePath'. Supply it with -BlazeCorePath."
    }
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $BlazeCorePath).Hash -ne $engineSha256) {
        throw "BlazeCore SHA-256 does not match APPROVED_ENGINE_SHA256: '$BlazeCorePath'."
    }

    if (-not $BlazeCoreLicensePath) {
        $candidates = @(
            (Join-Path (Split-Path -Parent $BlazeCorePath) 'LICENSE')
            (Join-Path $neighbor 'BlazeCore/LICENSE')
            (Join-Path $repo 'dist/windows/client/LICENSE-BLAZECORE')
        )
        $BlazeCoreLicensePath = $candidates | Where-Object {
            Test-Path -LiteralPath $_ -PathType Leaf
        } | Select-Object -First 1
        if (-not $BlazeCoreLicensePath) {
            throw 'BlazeCore LICENSE is missing. Supply it with -BlazeCoreLicensePath.'
        }
    }
    $BlazeCoreLicensePath = [IO.Path]::GetFullPath($BlazeCoreLicensePath, $workingDirectory)
    if (-not (Test-Path -LiteralPath $BlazeCoreLicensePath -PathType Leaf)) {
        throw "BlazeCore LICENSE is missing at '$BlazeCoreLicensePath'. Supply it with -BlazeCoreLicensePath."
    }
    @{ Engine = $BlazeCorePath; License = $BlazeCoreLicensePath; EngineHash = $engineSha256; DriverHash = $driverSha256 }
}

$enginePackage = Resolve-EnginePackage
$BlazeCorePath = $enginePackage.Engine
$BlazeCoreLicensePath = $enginePackage.License
$engineSha256 = $enginePackage.EngineHash
$driverSha256 = $enginePackage.DriverHash

$mutex = [Threading.Mutex]::new($false, 'Local\Blazer.WindowsPackage')
$locked = $false
try {
    try {
        $locked = $mutex.WaitOne(0)
    } catch [Threading.AbandonedMutexException] {
        $locked = $true
    }
    if (-not $locked) { throw 'Another Blazer Windows package build is running. Retry after it finishes.' }

    $directory = Join-Path $output 'client'
    Assert-PackageDirectory $directory
    Assert-PackageFileReplaceable (Join-Path $directory 'blazer.exe')

    Push-Location -LiteralPath $repo
    try {
        Build-Package
    } finally {
        Pop-Location
    }
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
