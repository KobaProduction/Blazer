#Requires -Version 7.0
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$PackageDirectory,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [Parameter(Mandatory)][string]$Version,
    [Parameter(Mandatory)][ValidateSet('preview','stable')][string]$Channel,
    [Parameter(Mandatory)][ValidatePattern('^[a-f0-9]{40}$')][string]$Commit,
    [Parameter(Mandatory)][ValidateRange(1,2147483647)][int]$Sequence,
    [Parameter(Mandatory)][hashtable[]]$Files,
    [string]$NsisCompiler,
    [switch]$Development
)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'toolchain.ps1')
. (Join-Path $PSScriptRoot 'paths.ps1')

if ($Version -cnotmatch '^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' -or
    @($Version.Split('.') | Where-Object { [uint64]$_ -gt 65535 }).Count) {
    throw 'Installer version must be X.Y with each Windows version component <= 65535.'
}
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$compiler = Get-BlazerNsisCompiler $NsisCompiler
$stageRoot = Join-Path $repo 'target/installer'
$stage = Join-Path $stageRoot ([Guid]::NewGuid().ToString('N'))
[void](Assert-BlazerRegularPath $stage $repo)
[IO.Directory]::CreateDirectory($stage) | Out-Null
$name = if ($Development) { "Blazer-development-$($Commit.Substring(0,12))-$Sequence-windows-x86_64-setup.exe" }
    elseif ($Channel -eq 'preview') { "Blazer-preview-$($Commit.Substring(0,12))-$Sequence-windows-x86_64-setup.exe" }
    else { "Blazer-$Version-windows-x86_64-setup.exe" }
$destination = Join-Path ([IO.Path]::GetFullPath($OutputDirectory)) $name
try {
    $payload = Join-Path $stage 'payload'
    [IO.Directory]::CreateDirectory($payload) | Out-Null
    $inventory = @()
    foreach ($file in $Files) {
        Assert-BlazerFileName $file.Name
        $source = Assert-BlazerRegularPath (Join-Path $PackageDirectory $file.Name) $PackageDirectory
        $target = Assert-BlazerRegularPath (Join-Path $payload $file.Name) $payload
        [IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
        [IO.File]::Copy($source, $target)
        $inventory += @{ path=$file.Name; sha256=(Get-FileHash -LiteralPath $target -Algorithm SHA256).Hash.ToLowerInvariant() }
    }
    $notice = Join-Path $payload 'licenses/nsis.txt'
    [IO.Directory]::CreateDirectory((Split-Path -Parent $notice)) | Out-Null
    Copy-Item -LiteralPath (Join-Path (Split-Path -Parent $compiler) 'COPYING') -Destination $notice
    $inventory += @{ path='licenses/nsis.txt'; sha256=(Get-FileHash -LiteralPath $notice -Algorithm SHA256).Hash.ToLowerInvariant() }
    $required = @('blazer.exe','blazecore.exe','wintun.dll','LICENSE','LICENSE-BLAZECORE','licenses/wintun-prebuilt.txt')
    foreach ($requiredName in $required) {
        if ($requiredName -cnotin @($inventory | ForEach-Object { $_.path })) { throw "Missing setup payload: $requiredName" }
    }
    Write-BlazerJson (Join-Path $payload 'payload.json') @{
        schema=1; product=$script:BlazerProduct; target='windows-x86_64'; version=$Version;
        channel=$Channel; sequence=$Sequence; commit=$Commit; files=$inventory }
    $temporary = Join-Path $stage $name
    $arguments = @('/V2','/WX','/INPUTCHARSET','UTF8',"/DAPP_VERSION=$Version","/DSETUP_OUTPUT=$temporary",
        "/DPAYLOAD_DIRECTORY=$payload", "/DAPP_ICON=$(Join-Path $repo 'src/platform/windows/assets/blazer.ico')",
        "/DAPP_LICENSE=$(Join-Path $repo 'LICENSE')", (Join-Path $PSScriptRoot 'blazer.nsi'))
    if ($Development) { $arguments = @('/DDEVELOPMENT_BUILD=1') + $arguments }
    & $compiler @arguments
    if ($LASTEXITCODE -ne 0) { throw "NSIS compilation failed (exit $LASTEXITCODE)." }
    [IO.Directory]::CreateDirectory((Split-Path -Parent $destination)) | Out-Null
    [IO.File]::Move($temporary, $destination, $true)
    Write-Output $destination
    Get-FileHash -LiteralPath $destination -Algorithm SHA256
} finally {
    [void](Assert-BlazerRegularPath $stage $stageRoot)
    if ([IO.Directory]::Exists($stage)) { [IO.Directory]::Delete($stage, $true) }
}
