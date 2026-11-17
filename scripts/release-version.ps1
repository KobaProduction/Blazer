function Get-ReleaseVersion {
    param([string]$Repository)

    $tags = @(git -C $Repository tag --points-at HEAD --list 'v*')
    if ($LASTEXITCODE -ne 0) { throw 'Could not find the Git release tag.' }
    $stableTags = @($tags | Where-Object { $_ -cmatch '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$' })
    if ($stableTags.Count -ne 1) { throw 'Release packaging needs exactly one stable vX.Y Git tag at HEAD.' }
    $changes = git -C $Repository status --porcelain --untracked-files=normal
    if ($LASTEXITCODE -ne 0 -or $changes) { throw 'Release packaging needs an unmodified checkout of the tagged commit.' }
    $stableTags[0].Substring(1)
}

function Get-WorkspaceAppVersion {
    param([string]$Repository, [string]$Package)

    Push-Location -LiteralPath $Repository
    try {
        $metadata = & cargo metadata --locked --no-deps --format-version 1
        if ($LASTEXITCODE -ne 0) { throw 'Could not read the workspace version from Cargo.' }
        $cargoVersion = (($metadata | ConvertFrom-Json).packages | Where-Object name -CEQ $Package).version
        if ($cargoVersion -cnotmatch '^(?<version>[0-9]+\.[0-9]+)\.[0-9]+$') { throw 'Unexpected Cargo workspace version.' }
        $Matches.version
    } finally {
        Pop-Location
    }
}
