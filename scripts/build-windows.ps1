#Requires -Version 7.0
param(
    [ValidateSet('Client', 'Demo', 'Both')]
    [string]$Variant = 'Both',
    [ValidateSet('Debug', 'Release')]
    [string]$Configuration = 'Debug',
    [string]$OutputDirectory,
    [string]$ShoesPath,
    [string]$ShoesLicensePath,
    [string]$WintunLicensePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
$wintunLicenseSha256 = '183ADAC21E7D96C508C8FD34D394B7B6708BC81564AD1BAD61AB66143A008CD2'

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
        [pscustomobject]@{
            Source = $file.Source
            ExpectedSha256 = $file['ExpectedSha256']
            Destination = Join-Path $Directory $file.Name
            Temporary = Join-Path $output ".blazer-$Name-$($file.Name).tmp"
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
        [IO.Directory]::CreateDirectory($Directory) | Out-Null
        foreach ($artifact in $staged) {
            [IO.File]::Move($artifact.Temporary, $artifact.Destination, $true)
        }
    } finally {
        foreach ($artifact in $artifacts) {
            [IO.File]::Delete($artifact.Temporary)
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
    param(
        [string]$Name,
        [string]$Binary,
        [string[]]$Features = @(),
        [string]$Engine,
        [string]$EngineLicense,
        [string]$EngineSha256,
        [string]$DriverSha256
    )

    & cargo build --locked --target $target --target-dir $targetDirectory -p vpn-client --bin $Binary @profileArguments @Features
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed for $Binary (exit $LASTEXITCODE)." }

    if ($Engine) {
        $driver = Join-Path $buildDirectory 'wintun.dll'
        if (-not (Test-Path -LiteralPath $driver -PathType Leaf)) {
            throw "The client build did not provide a Wintun DLL at '$driver'."
        }
        if ((Get-FileHash -Algorithm SHA256 -LiteralPath $driver).Hash -ne $DriverSha256) {
            throw "Wintun SHA-256 does not match APPROVED_WINTUN_SHA256: '$driver'."
        }
        $driverLicense = Resolve-WintunLicense
    }

    $directory = Join-Path $output $Name
    $files = @(
        @{ Name = 'LICENSE'; Source = Join-Path $repo 'LICENSE' }
        @{ Name = 'LICENSE-NOTO-EMOJI'; Source = Join-Path $repo 'src/presentation/desktop/assets/LICENSE-NOTO-EMOJI.txt' }
    )
    if ($Engine) {
        $files += @(
            @{ Name = 'LICENSE-SHOES'; Source = $EngineLicense }
            @{ Name = 'shoes.exe'; Source = $Engine; ExpectedSha256 = $EngineSha256 }
            @{ Name = 'wintun.dll'; Source = $driver; ExpectedSha256 = $DriverSha256 }
            @{ Name = 'LICENSE-WINTUN'; Source = $driverLicense; ExpectedSha256 = $wintunLicenseSha256 }
        )
    }
    $executable = Join-Path $directory "$Binary.exe"
    Assert-PackageDirectory $directory
    Assert-PackageFileReplaceable $executable
    $app = @{ Name = "$Binary.exe"; Source = Join-Path $buildDirectory "$Binary.exe" }
    # For an existing package, replace the executable first so a late launch blocks publication.
    # A fresh package becomes launchable only after its companion files are installed.
    $files = if ([IO.File]::Exists($executable)) { @($app) + $files } else { $files + @($app) }
    Publish-Package $Name $directory $files
    Write-Output $executable
}

$repo = Split-Path -Parent $PSScriptRoot
$target = 'x86_64-pc-windows-msvc'
$targetDirectory = Join-Path $repo 'target'
$targetPlatformDirectory = Join-Path $targetDirectory $target
$buildDirectory = Join-Path $targetPlatformDirectory $Configuration.ToLowerInvariant()
$profileArguments = @()
if ($Configuration -eq 'Release') { $profileArguments = @('--release') }
$workingDirectory = (Get-Location).ProviderPath
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo 'dist/windows' }
$output = [IO.Path]::GetFullPath($OutputDirectory, $workingDirectory)

if ($Variant -ne 'Demo') {
    if (-not $ShoesPath) { $ShoesPath = Join-Path $targetPlatformDirectory 'release/shoes.exe' }
    $ShoesPath = [IO.Path]::GetFullPath($ShoesPath, $workingDirectory)
    if (-not (Test-Path -LiteralPath $ShoesPath -PathType Leaf)) {
        throw "Approved Shoes executable is missing at '$ShoesPath'. Supply it with -ShoesPath."
    }
    $engineSource = Join-Path $repo 'src/infrastructure/client_engine'
    $engineSha256 = Get-ApprovedSha256 (Join-Path $engineSource 'mod.rs') 'APPROVED_ENGINE_SHA256'
    $driverSha256 = Get-ApprovedSha256 (Join-Path $engineSource 'windows_tunnel.rs') 'APPROVED_WINTUN_SHA256'
    if ((Get-FileHash -Algorithm SHA256 -LiteralPath $ShoesPath).Hash -ne $engineSha256) {
        throw "Shoes SHA-256 does not match APPROVED_ENGINE_SHA256: '$ShoesPath'."
    }

    if (-not $ShoesLicensePath) {
        $neighbor = Split-Path -Parent $repo
        $candidates = @(
            (Join-Path $neighbor 'BlazeCore/LICENSE')
            (Join-Path $neighbor 'shoes/LICENSE')
            (Join-Path $repo 'dist/windows/client/LICENSE-SHOES')
        )
        $ShoesLicensePath = $candidates | Where-Object {
            Test-Path -LiteralPath $_ -PathType Leaf
        } | Select-Object -First 1
        if (-not $ShoesLicensePath) {
            throw 'Shoes LICENSE is missing. Supply it with -ShoesLicensePath.'
        }
    }
    $ShoesLicensePath = [IO.Path]::GetFullPath($ShoesLicensePath, $workingDirectory)
    if (-not (Test-Path -LiteralPath $ShoesLicensePath -PathType Leaf)) {
        throw "Shoes LICENSE is missing at '$ShoesLicensePath'. Supply it with -ShoesLicensePath."
    }
}

$mutex = [Threading.Mutex]::new($false, 'Local\Blazer.WindowsPackage')
$locked = $false
try {
    try {
        $locked = $mutex.WaitOne(0)
    } catch [Threading.AbandonedMutexException] {
        $locked = $true
    }
    if (-not $locked) { throw 'Another Blazer Windows package build is running. Retry after it finishes.' }

    foreach ($package in @(
        @{ Name = 'client'; Binary = 'vpn-client.exe'; Selected = $Variant -ne 'Demo' }
        @{ Name = 'demo'; Binary = 'vpn-client-demo.exe'; Selected = $Variant -ne 'Client' }
    )) {
        if ($package.Selected) {
            $directory = Join-Path $output $package.Name
            Assert-PackageDirectory $directory
            Assert-PackageFileReplaceable (Join-Path $directory $package.Binary)
        }
    }

    Push-Location -LiteralPath $repo
    try {
        if ($Variant -ne 'Demo') {
            Build-Package -Name client -Binary vpn-client -Engine $ShoesPath -EngineLicense $ShoesLicensePath -EngineSha256 $engineSha256 -DriverSha256 $driverSha256
        }
        if ($Variant -ne 'Client') {
            Build-Package -Name demo -Binary vpn-client-demo -Features @('--no-default-features', '--features', 'demo')
        }
    } finally {
        Pop-Location
    }
} finally {
    if ($locked) { $mutex.ReleaseMutex() }
    $mutex.Dispose()
}
