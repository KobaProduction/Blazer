param(
    [string]$SdkRoot = $env:ANDROID_HOME,
    [string]$JavaHome = $env:JAVA_HOME,
    [string]$NdkVersion = '27.2.12479018',
    [string]$NdkRoot,
    [string]$BuildToolsVersion = '35.0.1',
    [ValidateSet('x86_64', 'arm64-v8a')]
    [string[]]$Abi = @('x86_64', 'arm64-v8a'),
    [string]$BlazeCoreLibraries,
    [switch]$TaggedVersion,
    [ValidateSet('Development', 'Direct')]
    [string]$Distribution = 'Development',
    [ValidateRange(1, 2100000000)]
    [int]$VersionCode,
    [string]$SigningKeyStore,
    [string]$SigningKeyAlias,
    [string]$OutputPath = 'target/android/blazer-debug.apk'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
. (Join-Path $PSScriptRoot 'release-version.ps1')

function Invoke-Step {
    param([string]$Executable, [string[]]$Arguments)
    $start = [Diagnostics.ProcessStartInfo]::new($Executable)
    $start.WorkingDirectory = (Get-Location).Path
    $start.UseShellExecute = $false
    $start.CreateNoWindow = $true
    $start.RedirectStandardOutput = $true
    $start.RedirectStandardError = $true
    foreach ($argument in $Arguments) { $start.ArgumentList.Add($argument) }
    $process = [Diagnostics.Process]::Start($start)
    try {
        $output = $process.StandardOutput.ReadToEndAsync()
        $errors = $process.StandardError.ReadToEndAsync()
        $process.WaitForExit()
        if ($output.Result) { Write-Host $output.Result.TrimEnd() }
        if ($errors.Result) { Write-Host $errors.Result.TrimEnd() }
        if ($process.ExitCode -ne 0) {
            throw "Build step failed: $Executable (exit $($process.ExitCode))"
        }
    } finally {
        $process.Dispose()
    }
}

function Resolve-Tool {
    param([string]$Path)
    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
        throw "Required build tool is missing: $Path"
    }
    (Resolve-Path -LiteralPath $Path).Path
}

function New-BuildStage {
    param([string]$Parent, [string]$Prefix)
    $path = Join-Path $Parent ($Prefix + [Guid]::NewGuid().ToString('N'))
    (New-Item -ItemType Directory -Path $path).FullName
}

function Remove-BuildStage {
    param([string]$Path, [string]$Parent)
    if (-not $Path) { return }

    $parentDirectory = (Get-Item -LiteralPath $Parent).FullName
    $stageDirectory = Get-Item -LiteralPath $Path -Force
    if (-not $stageDirectory.PSIsContainer -or
        $stageDirectory.Parent.FullName -ne $parentDirectory -or
        $stageDirectory.Name -notmatch '^(package-|\.blazer-sign-)[0-9a-f]{32}$' -or
        ($stageDirectory.Attributes -band [IO.FileAttributes]::ReparsePoint)) {
        throw "Refusing to remove a directory outside the current build stage: $Path"
    }
    Remove-Item -LiteralPath $stageDirectory.FullName -Force -Recurse
}

function Move-VerifiedApk {
    param([string]$Signed, [string]$Output)

    $signature = "$Output.idsig"
    $previousSignature = "$Signed.previous.idsig"
    # Remove the previous signature from view before replacing the APK it describes.
    if (Test-Path -LiteralPath $signature) {
        [IO.File]::Move($signature, $previousSignature)
    }
    try {
        [IO.File]::Move($Signed, $Output, $true)
    } catch {
        if (Test-Path -LiteralPath $previousSignature -PathType Leaf) {
            [IO.File]::Move($previousSignature, $signature)
        }
        throw
    }
    if (Test-Path -LiteralPath "$Signed.idsig" -PathType Leaf) {
        [IO.File]::Move("$Signed.idsig", $signature)
    }
}

function Publish-VerifiedApk {
    param([string]$Signed, [string]$Output)

    $pathBytes = [Text.Encoding]::UTF8.GetBytes($Output.ToUpperInvariant())
    $digest = [Convert]::ToHexString([Security.Cryptography.SHA256]::HashData($pathBytes))
    $publishLock = [Threading.Mutex]::new($false, "Global\Blazer.Android.Apk.$digest")
    $locked = $false
    try {
        try {
            $locked = $publishLock.WaitOne([TimeSpan]::FromSeconds(15))
        } catch [Threading.AbandonedMutexException] {
            $locked = $true
        }
        if (-not $locked) { throw 'Another build is publishing to the same APK output.' }
        Move-VerifiedApk $Signed $Output
    } finally {
        if ($locked) { $publishLock.ReleaseMutex() }
        $publishLock.Dispose()
    }
}

if (-not $SdkRoot -or -not $JavaHome) { throw 'Set ANDROID_HOME and JAVA_HOME, or supply -SdkRoot and -JavaHome.' }
$sdk = (Resolve-Path -LiteralPath $SdkRoot).Path
$env:JAVA_HOME = (Resolve-Path -LiteralPath $JavaHome).Path
$env:PATH = (Join-Path $env:JAVA_HOME 'bin') + [IO.Path]::PathSeparator + $env:PATH
$tools = Join-Path $sdk "build-tools/$BuildToolsVersion"
if (-not $NdkRoot) { $NdkRoot = Join-Path $sdk "ndk/$NdkVersion" }
$ndk = Join-Path (Resolve-Path -LiteralPath $NdkRoot).Path 'toolchains/llvm/prebuilt/windows-x86_64/bin'
$clang = Resolve-Tool (Join-Path $ndk 'clang.exe')
$strip = Resolve-Tool (Join-Path $ndk 'llvm-strip.exe')
$archiver = Resolve-Tool (Join-Path $ndk 'llvm-ar.exe')
$javac = Resolve-Tool (Join-Path $env:JAVA_HOME 'bin/javac.exe')
$java = Resolve-Tool (Join-Path $env:JAVA_HOME 'bin/java.exe')
$keytool = Resolve-Tool (Join-Path $env:JAVA_HOME 'bin/keytool.exe')
$androidJar = Resolve-Tool (Join-Path $sdk 'platforms/android-35/android.jar')
$aapt = Resolve-Tool (Join-Path $tools 'aapt.exe')
$aapt2 = Resolve-Tool (Join-Path $tools 'aapt2.exe')
$d8 = Resolve-Tool (Join-Path $tools 'lib/d8.jar')
$zipalign = Resolve-Tool (Join-Path $tools 'zipalign.exe')
$signer = Resolve-Tool (Join-Path $tools 'lib/apksigner.jar')
$repo = Split-Path -Parent $PSScriptRoot
$direct = $Distribution -eq 'Direct'
$releaseVersion = if ($TaggedVersion -or $direct) { Get-ReleaseVersion $repo } else { $null }
$keyAlias = 'androiddebugkey'
$signingPasswords = @('--ks-pass', 'pass:android', '--key-pass', 'pass:android')
if ($direct) {
    if (-not $SigningKeyStore -or -not $SigningKeyAlias -or -not $env:BLAZER_ANDROID_STORE_PASSWORD) {
        throw 'Direct APK signing requires SigningKeyStore, SigningKeyAlias and BLAZER_ANDROID_STORE_PASSWORD.'
    }
    $SigningKeyStore = (Resolve-Path -LiteralPath $SigningKeyStore).Path
    $keyAlias = $SigningKeyAlias
    $keyPassword = if ($env:BLAZER_ANDROID_KEY_PASSWORD) { 'BLAZER_ANDROID_KEY_PASSWORD' } else { 'BLAZER_ANDROID_STORE_PASSWORD' }
    $signingPasswords = @('--ks-pass', 'env:BLAZER_ANDROID_STORE_PASSWORD', '--key-pass', "env:$keyPassword")
    if (-not $PSBoundParameters.ContainsKey('OutputPath')) { $OutputPath = "target/android/Blazer-$releaseVersion-android.apk" }
}
$package = Join-Path $repo 'src/platform/android/packaging'
$absOutput = [IO.Path]::GetFullPath($OutputPath, $repo)
$work = Join-Path $repo 'target/android'
$outputDirectory = Split-Path -Parent $absOutput
$targets = @{ 'x86_64' = 'x86_64-linux-android'; 'arm64-v8a' = 'aarch64-linux-android' }
$manifest = Join-Path $package 'AndroidManifest.xml'
$keystore = if ($direct) { $SigningKeyStore } else { Join-Path $work 'local-debug.keystore' }
if ($absOutput -eq $keystore) { throw 'The APK output cannot replace the debug keystore.' }
$blazeCoreHashes = $null
if ($BlazeCoreLibraries) {
    $BlazeCoreLibraries = (Resolve-Path -LiteralPath $BlazeCoreLibraries).Path
    $provenance = Get-Content -Raw -LiteralPath (Join-Path $BlazeCoreLibraries 'manifest.json') | ConvertFrom-Json -AsHashtable
    if ($provenance['revision'] -ne 'cce97a2c4c1c3bd0a07807f37a7caf3876c79110' -or $provenance['version'] -ne '0.3.7') {
        throw 'The Android BlazeCore bundle does not target the approved revision and version.'
    }
    $blazeCoreHashes = $provenance['sha256']
    if (-not $blazeCoreHashes) { throw 'Missing Android BlazeCore bundle digests.' }
}

$stage = $null
$signingStage = $null
$previousVersion = $env:BLAZER_RELEASE_VERSION
$previousUserAgent = $env:BLAZER_RELEASE_USER_AGENT
Push-Location $repo
try {
    $env:BLAZER_RELEASE_VERSION = $releaseVersion
    $env:BLAZER_RELEASE_USER_AGENT = if ($releaseVersion) { "Blazer/$releaseVersion" } else { $null }
    $version = $releaseVersion
    if (-not $version) {
        $version = Get-WorkspaceAppVersion $repo 'vpn-android'
    }
    if ($version -cnotmatch '^(?<major>0|[1-9][0-9]{0,3})\.(?<minor>0|[1-9][0-9]{0,2})$' -or
        [int]$Matches.major -gt 2099) {
        throw "Android APK requires a numeric version X.Y (X <= 2099, Y <= 999): '$version'."
    }
    $apkVersionCode = if ($VersionCode) { $VersionCode } else { 1 + [int]$Matches.major * 1000000 + [int]$Matches.minor * 1000 }
    New-Item -ItemType Directory -Force -Path $work, $outputDirectory | Out-Null
    $stage = New-BuildStage $work 'package-'
    $linkVariant = @('--debug-mode')
    if ($direct) {
        $applicationManifest = [xml](Get-Content -Raw -LiteralPath $manifest)
        $namespaces = [System.Xml.XmlNamespaceManager]::new($applicationManifest.NameTable)
        $namespaces.AddNamespace('android', 'http://schemas.android.com/apk/res/android')
        $channel = $applicationManifest.SelectSingleNode('/manifest/application/meta-data[@android:name="com.kobaproduction.blazer.update_channel"]', $namespaces)
        $channel.Attributes['android:value'].Value = 'direct'
        $manifest = Join-Path $stage 'AndroidManifest.xml'
        $applicationManifest.Save($manifest)
        $linkVariant = @()
    }
    $classes = Join-Path $stage 'classes'
    $generatedSources = Join-Path $stage 'generated'
    New-Item -ItemType Directory -Path $classes, $generatedSources | Out-Null
    $resources = Join-Path $stage 'resources.zip'
    $unsigned = Join-Path $stage 'unsigned.apk'
    $aligned = Join-Path $stage 'aligned.apk'

    $apkEntries = @('classes.dex')
    foreach ($platform in $Abi | Select-Object -Unique) {
        $target = $targets[$platform]
        $envTarget = $target.Replace('-', '_')
        $upperTarget = $envTarget.ToUpperInvariant()
        Set-Item -Path "Env:CARGO_TARGET_${upperTarget}_LINKER" -Value $clang
        Set-Item -Path "Env:CARGO_TARGET_${upperTarget}_RUSTFLAGS" -Value "-C link-arg=--target=${target}26 -C link-arg=-Wl,-z,max-page-size=16384"
        Set-Item -Path "Env:CC_$envTarget" -Value $clang
        Set-Item -Path "Env:AR_$envTarget" -Value $archiver
        Set-Item -Path "Env:CFLAGS_$envTarget" -Value "--target=${target}26"

        Invoke-Step cargo @('build', '--locked', '--release', '-p', 'vpn-android', '--target', $target)
        $entry = "lib/$platform/libblazer_android.so"
        $destination = Join-Path $stage $entry
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
        Copy-Item -LiteralPath (Join-Path $repo "target/$target/release/libblazer_android.so") -Destination $destination
        Invoke-Step $strip @('--strip-debug', $destination)
        $apkEntries += $entry
        if ($blazeCoreHashes) {
            $engineEntry = "lib/$platform/libblazecore.so"
            $engine = Resolve-Tool (Join-Path $BlazeCoreLibraries "$platform/libblazecore.so")
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $engine).Hash -ne $blazeCoreHashes[$platform]) {
                throw "Android BlazeCore digest mismatch for $platform."
            }
            Copy-Item -LiteralPath $engine -Destination (Join-Path $stage $engineEntry)
            $apkEntries += $engineEntry
        }
    }

    Invoke-Step $aapt2 @('compile', '--dir', (Join-Path $package 'res'), '-o', $resources)
    Invoke-Step $aapt2 (@('link', '-I', $androidJar, '--manifest', $manifest) + $linkVariant + @(
        '--version-name', $version, '--version-code', $apkVersionCode.ToString([Globalization.CultureInfo]::InvariantCulture),
        '--java', $generatedSources, '-o', $unsigned, $resources))
    $sources = @(Get-ChildItem -LiteralPath (Join-Path $package 'java'), $generatedSources -Recurse -Filter '*.java' -File |
        ForEach-Object { $_.FullName })
    Invoke-Step $javac (@('--release', '17', '-encoding', 'UTF-8', '-classpath', $androidJar, '-d', $classes) + $sources)
    $code = Join-Path $stage 'classes.zip'
    [IO.Compression.ZipFile]::CreateFromDirectory($classes, $code)
    Invoke-Step $java @('-cp', $d8, 'com.android.tools.r8.D8', '--min-api', '26', '--lib', $androidJar, '--output', $stage, $code)

    Push-Location $stage
    try {
        Invoke-Step $aapt (@('add', $unsigned) + $apkEntries)
    } finally {
        Pop-Location
    }
    Invoke-Step $zipalign @('-P', '16', '-f', '4', $unsigned, $aligned)
    if (-not $direct -and -not (Test-Path -LiteralPath $keystore -PathType Leaf)) {
        $newKeystore = Join-Path $stage 'local-debug.keystore'
        Invoke-Step $keytool @('-genkeypair', '-keystore', $newKeystore, '-storepass', 'android',
            '-keypass', 'android', '-alias', 'androiddebugkey', '-keyalg', 'RSA', '-keysize', '2048',
            '-validity', '3650', '-dname', 'CN=Android Debug,O=Android,C=US')
        try {
            [IO.File]::Move($newKeystore, $keystore)
        } catch [IO.IOException] {
            # Another build may have published its completed key first.
            if (-not (Test-Path -LiteralPath $keystore -PathType Leaf)) { throw }
        }
    }

    # Stage next to the destination so the final move stays on the same volume.
    $signingStage = New-BuildStage $outputDirectory '.blazer-sign-'
    $signed = Join-Path $signingStage 'signed.apk'
    Invoke-Step $java (@('-jar', $signer, 'sign', '--ks', $keystore, '--ks-key-alias', $keyAlias) +
        $signingPasswords + @('--out', $signed, $aligned))
    $verify = @('-jar', $signer, 'verify', '--verbose')
    if (Test-Path -LiteralPath "$signed.idsig" -PathType Leaf) {
        $verify += @('--v4-signature-file', "$signed.idsig")
    }
    Invoke-Step $java ($verify + $signed)
    Publish-VerifiedApk $signed $absOutput
    Write-Output $absOutput
} finally {
    $env:BLAZER_RELEASE_VERSION = $previousVersion
    $env:BLAZER_RELEASE_USER_AGENT = $previousUserAgent
    Pop-Location
    try {
        Remove-BuildStage $signingStage $outputDirectory
    } finally {
        Remove-BuildStage $stage $work
    }
}
