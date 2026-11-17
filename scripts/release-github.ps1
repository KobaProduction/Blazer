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

function Publish-Preview {
    $current = Invoke-Gh @('api', "repos/$repository/git/ref/heads/main", '--jq', '.object.sha')
    if ($current -cne $head) { Write-Host 'A newer main commit is queued for publication.'; return }

    $release = Invoke-Gh @('api', "repos/$repository/releases/tags/preview") | ConvertFrom-Json
    $immutable = $release.PSObject.Properties['immutable']
    if (-not $release.prerelease -or $release.draft -or ($immutable -and $immutable.Value)) {
        throw 'Expected a mutable published prerelease at preview.'
    }
    $assetName = "Blazer-preview-$short-$env:GITHUB_RUN_ID-$env:GITHUB_RUN_ATTEMPT-windows-x86_64.zip"
    $upload = Join-Path $output $assetName
    Copy-Item -LiteralPath $archivePath -Destination $upload
    Invoke-Gh @('release', 'upload', 'preview', $upload, '--repo', $repository)

    $expectedDigest = 'sha256:' + (Get-FileHash -LiteralPath $upload -Algorithm SHA256).Hash.ToLowerInvariant()
    $release = Invoke-Gh @('api', "repos/$repository/releases/tags/preview") | ConvertFrom-Json
    $uploaded = $release.assets | Where-Object name -CEQ $assetName
    if (-not $uploaded -or $uploaded.digest -cne $expectedDigest) { throw 'The uploaded preview SHA-256 did not match.' }

    # Move the tag only after the successor is uploaded and verified.
    Invoke-Gh @('api', '--method', 'PATCH', "repos/$repository/git/refs/tags/preview", '-f', "sha=$head", '-F', 'force=true') | Out-Null
    $notes = "Сборка $version из main, коммит $head. GitHub Actions: $env:GITHUB_RUN_NUMBER.$env:GITHUB_RUN_ATTEMPT."
    Invoke-Gh @('release', 'edit', 'preview', '--title', "Blazer $version prerelease ($short)", '--notes', $notes,
        '--prerelease', '--latest=false', '--repo', $repository)
    foreach ($old in $release.assets) {
        if ($old.name -cmatch $previewAsset -and $old.name -cne $assetName) {
            Invoke-Gh @('release', 'delete-asset', 'preview', $old.name, '--yes', '--repo', $repository)
        }
    }
}

$repo = Split-Path $PSScriptRoot -Parent
$repository = $env:GITHUB_REPOSITORY
if ($repository -cne 'KobaProduction/Blazer' -or -not $env:RUNNER_TEMP) { throw 'Run this script in the Blazer release workflow.' }
$head = git -C $repo rev-parse HEAD
if ($LASTEXITCODE -ne 0 -or $head -notmatch '^[a-f0-9]{40}$') { throw 'Could not read the build commit.' }
$short = $head.Substring(0, 12)
$preview = $env:GITHUB_REF -ceq 'refs/heads/main'
$version = if ($preview) { Get-WorkspaceAppVersion $repo 'blazer' } else { Get-ReleaseVersion $repo }
if (-not $preview -and $env:GITHUB_REF -cne "refs/tags/v$version") { throw 'The stable release must run on its vX.Y tag.' }
$api = "https://api.github.com/repos/$repository"
$readHeaders = @{ Accept = 'application/vnd.github+json'; 'User-Agent' = 'Blazer-release-build' }
$previewAsset = '^Blazer-preview-[a-f0-9]{12}-(bootstrap|[0-9]+-[0-9]+)-windows-x86_64\.zip$'
$output = Join-Path $env:RUNNER_TEMP 'blazer-release-output'
$name = if ($preview) { 'Blazer-preview-windows-x86_64.zip' } else { "Blazer-$version-windows-x86_64.zip" }
$archivePath = Join-Path $output $name

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
} elseif ($preview) {
    Publish-Preview
} else {
    # Tag creation is explicit. Publishing this package leaves the tagged source intact.
    Invoke-Gh @('release', 'create', "v$version", $archivePath, '--verify-tag', '--title', "Blazer $version", '--generate-notes', '--repo', $repository)
}
