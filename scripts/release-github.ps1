#Requires -Version 7.0
param([Parameter(Mandatory)][ValidateSet('Build', 'Publish')][string]$Stage)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'release-version.ps1')

function Invoke-Gh {
    param([string[]]$Arguments)
    $result = & gh @Arguments
    if ($LASTEXITCODE -ne 0) { throw "GitHub CLI failed: $($Arguments[0]) $($Arguments[1])" }
    $result
}

function Find-EngineArchive {
    if ($env:BLAZER_WINDOWS_ENGINE_ARCHIVE_URL) {
        $configured = [uri]$env:BLAZER_WINDOWS_ENGINE_ARCHIVE_URL
        if (-not $configured.IsAbsoluteUri -or $configured.Scheme -ne 'https' -or $configured.UserInfo) {
            throw 'BLAZER_WINDOWS_ENGINE_ARCHIVE_URL must be an HTTPS URL without credentials.'
        }
        return $configured.AbsoluteUri
    }
    # The first preview is seeded from the locally approved core. A queued runner can race that upload.
    for ($attempt = 0; $attempt -lt 12; $attempt++) {
        $response = Invoke-WebRequest -Uri "$api/releases/tags/preview" -Headers $readHeaders -SkipHttpErrorCheck -TimeoutSec 15
        if ($response.StatusCode -notin @(200, 404)) { throw "GitHub release lookup returned HTTP $($response.StatusCode)." }
        if ($response.StatusCode -eq 200) {
            $release = $response.Content | ConvertFrom-Json
            $asset = $release.assets | Where-Object { $_.name -cmatch $previewAsset -and $_.state -eq 'uploaded' } |
                Sort-Object created_at -Descending | Select-Object -First 1
            if ($asset) { return $asset.browser_download_url }
        }
        if ($attempt -lt 11) { Start-Sleep -Seconds 10 }
    }
    throw 'The approved BlazeCore seed is missing. Publish the initial local preview archive or set BLAZER_WINDOWS_ENGINE_ARCHIVE_URL.'
}

function Expand-EngineFiles {
    param([string]$Archive, [string]$Destination)

    [IO.Directory]::CreateDirectory($Destination) | Out-Null
    $zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    try {
        foreach ($name in 'blazecore.exe', 'LICENSE-BLAZECORE', 'THIRD_PARTY_NOTICES.md') {
            $entry = $zip.GetEntry($name)
            if ($entry) {
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $Destination $name), $true)
            } elseif ($name -ne 'THIRD_PARTY_NOTICES.md') {
                throw "The BlazeCore archive is missing $name."
            }
        }
        foreach ($entry in $zip.Entries) {
            if ($entry.FullName -cmatch '^licenses/[A-Za-z0-9][A-Za-z0-9._-]*$') {
                [IO.Directory]::CreateDirectory((Join-Path $Destination 'licenses')) | Out-Null
                [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, (Join-Path $Destination $entry.FullName), $true)
            }
        }
    } finally {
        $zip.Dispose()
    }
}

function Assert-UploadedAssets {
    param([string[]]$Paths)
    $release = Invoke-Gh @('api', "repos/$repository/releases/tags/preview") | ConvertFrom-Json
    foreach ($upload in $Paths) {
        $assetName = Split-Path -Leaf $upload
        $expectedDigest = 'sha256:' + (Get-FileHash -LiteralPath $upload -Algorithm SHA256).Hash.ToLowerInvariant()
        $uploaded = @($release.assets | Where-Object name -CEQ $assetName)
        if ($uploaded.Count -ne 1 -or $uploaded[0].digest -cne $expectedDigest) {
            throw "The uploaded preview SHA-256 did not match: $assetName"
        }
    }
    $release
}

function Publish-Preview {
    $current = Invoke-Gh @('api', "repos/$repository/git/ref/heads/main", '--jq', '.object.sha')
    if ($current -cne $head) { Write-Host 'A newer main commit is queued for publication.'; return }

    $release = Invoke-Gh @('api', "repos/$repository/releases/tags/preview") | ConvertFrom-Json
    $immutable = $release.PSObject.Properties['immutable']
    if (-not $release.prerelease -or $release.draft -or ($immutable -and $immutable.Value)) {
        throw 'Expected a mutable published prerelease at preview.'
    }
    $seedName = "Blazer-preview-$short-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT-windows-x86_64.zip"
    $seedUpload = Join-Path $output $seedName
    Copy-Item -LiteralPath $archivePath -Destination $seedUpload
    $uploads = @($setupPath, $seedUpload)
    Invoke-Gh (@('release', 'upload', 'preview') + $uploads + @('--clobber', '--repo', $repository))
    Assert-UploadedAssets $uploads | Out-Null
    # Metadata is the updater handoff; publish it only after its payload is verified.
    Invoke-Gh @('release', 'upload', 'preview', $metadataPath, '--clobber', '--repo', $repository)
    $release = Assert-UploadedAssets @($metadataPath)

    # Move the tag only after the successor is uploaded and verified.
    Invoke-Gh @('api', '--method', 'PATCH', "repos/$repository/git/refs/tags/preview", '-f', "sha=$head", '-F', 'force=true') | Out-Null
    $notes = "Установщик Blazer $version, preview $sequence из main, коммит $head. GitHub Actions: $env:GITHUB_RUN_NUMBER.$env:GITHUB_RUN_ATTEMPT. ZIP — внутренний комплект закреплённого ядра для следующей сборки."
    Invoke-Gh @('release', 'edit', 'preview', '--title', "Blazer $version prerelease ($short)", '--notes', $notes,
        '--prerelease', '--latest=false', '--repo', $repository)
    foreach ($old in $release.assets) {
        if (($old.name -cmatch $previewAsset -or $old.name -cmatch $previewInstallerAsset) -and
            $old.name -cnotin @($setupName, $metadataName, $seedName)) {
            Invoke-Gh @('release', 'delete-asset', 'preview', $old.name, '--yes', '--repo', $repository)
        }
    }
}

function Write-InstallerMetadata {
    if (-not (Test-Path -LiteralPath $setupPath -PathType Leaf)) { throw 'Windows setup executable is missing.' }
    $setup = Get-Item -LiteralPath $setupPath
    if ($setup.Length -le 0 -or $setup.Length -gt 256MB) { throw 'Windows setup exceeds the update size limit.' }
    $metadata = @{
        schema = 1
        version = $version
        channel = if ($preview) { 'preview' } else { 'stable' }
        sequence = $sequence
        commit = $head
        setup = @{
            name = $setupName
            size = $setup.Length
            sha256 = (Get-FileHash -LiteralPath $setupPath -Algorithm SHA256).Hash.ToLowerInvariant()
        }
    }
    [IO.File]::WriteAllText($metadataPath, ($metadata | ConvertTo-Json -Depth 3 -Compress))
}

$repo = Split-Path $PSScriptRoot -Parent
$repository = $env:GITHUB_REPOSITORY
if ($repository -cne 'KobaProduction/Blazer' -or -not $env:RUNNER_TEMP) { throw 'Run this script in the Blazer release workflow.' }
$head = git -C $repo rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $head -notmatch '^[a-f0-9]{40}$') { throw 'Could not read the build commit.' }
$short = $head.Substring(0, 12)
$sequenceText = git -C $repo rev-list --first-parent --count HEAD
if ($LASTEXITCODE -ne 0 -or $sequenceText -notmatch '^[1-9][0-9]*$' -or [long]$sequenceText -gt [int]::MaxValue) {
    throw 'Could not read the bounded Git first-parent build sequence.'
}
$sequence = [int]$sequenceText
$changes = git -C $repo status --porcelain --untracked-files=normal
if ($LASTEXITCODE -ne 0 -or $changes) { throw 'Release automation requires a clean source checkout.' }
$preview = $env:GITHUB_REF -ceq 'refs/heads/main'
$version = if ($preview) { Get-WorkspaceAppVersion $repo 'blazer' } else { Get-ReleaseVersion $repo }
if (-not $preview -and $env:GITHUB_REF -cne "refs/tags/v$version") { throw 'The stable release must run on its vX.Y tag.' }
$api = "https://api.github.com/repos/$repository"
$readHeaders = @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'Blazer-release-build' }
$previewAsset = '^Blazer-preview-[a-f0-9]{12}-(bootstrap|[0-9]+-[0-9]+)-windows-x86_64\.zip$'
$previewInstallerAsset = '^Blazer-preview-[a-f0-9]{12}-[1-9][0-9]*-windows-x86_64-(setup\.exe|update\.json)$'
$output = Join-Path $env:RUNNER_TEMP 'blazer-release-output'
$name = if ($preview) { 'Blazer-preview-windows-x86_64.zip' } else { "Blazer-$version-windows-x86_64.zip" }
$archivePath = Join-Path $output $name
$stem = if ($preview) { "Blazer-preview-$short-$sequence-windows-x86_64" } else { "Blazer-$version-windows-x86_64" }
$setupName = "$stem-setup.exe"
$metadataName = "$stem-update.json"
$setupPath = Join-Path $output $setupName
$metadataPath = Join-Path $output $metadataName

if ($Stage -eq 'Build') {
    $cache = Join-Path $env:RUNNER_TEMP 'blazer-engine-input'
    [IO.Directory]::CreateDirectory($cache) | Out-Null
    $engineArchive = Join-Path $cache 'engine.zip'
    Invoke-WebRequest -Uri (Find-EngineArchive) -OutFile $engineArchive -TimeoutSec 180
    $engine = Join-Path $cache 'engine'
    Expand-EngineFiles $engineArchive $engine
    $options = @{
        Configuration = 'Release'
        OutputDirectory = $output
        BlazeCorePath = Join-Path $engine 'blazecore.exe'
        BlazeCoreLicensePath = Join-Path $engine 'LICENSE-BLAZECORE'
    }
    if ($preview) { $options.PreviewArchive = $true } else { $options.ReleaseArchive = $true }
    & "$PSScriptRoot/build-windows.ps1" @options
    if (-not (Test-Path -LiteralPath $archivePath -PathType Leaf)) { throw 'Windows release archive is missing.' }
    Write-InstallerMetadata
} elseif ($preview) {
    Publish-Preview
} else {
    # Tag creation is explicit. Publishing this package leaves the tagged source intact.
    Invoke-Gh @('release', 'create', "v$version", $setupPath, $metadataPath, $archivePath, '--verify-tag', '--title', "Blazer $version", '--generate-notes', '--repo', $repository)
}
