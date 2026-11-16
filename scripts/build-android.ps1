param(
    [string]$SdkRoot = $env:ANDROID_HOME,
    [string]$JavaHome = $env:JAVA_HOME,
    [string]$NdkVersion = '27.2.12479018',
    [string]$NdkRoot,
    [string]$BuildToolsVersion = '35.0.1',
    [ValidateSet('x86_64', 'arm64-v8a')]
    [string[]]$Abi = @('x86_64', 'arm64-v8a'),
    [string]$ShoesLibraries,
    [string]$OutputPath = 'target/android/vpn-client-debug.apk'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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
        $stageDirectory.Name -notmatch '^(package-|\.vpn-client-sign-)[0-9a-f]{32}$' -or
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
$package = Join-Path $repo 'src/presentation/android/packaging'
$absOutput = [IO.Path]::GetFullPath($OutputPath, $repo)
$work = Join-Path $repo 'target/android'
$outputDirectory = Split-Path -Parent $absOutput
$targets = @{ 'x86_64' = 'x86_64-linux-android'; 'arm64-v8a' = 'aarch64-linux-android' }
$manifest = Join-Path $package 'AndroidManifest.xml'
$keystore = Join-Path $work 'local-debug.keystore'
if ($absOutput -eq $keystore) { throw 'The APK output cannot replace the debug keystore.' }
$shoesHashes = $null
if ($ShoesLibraries) {
    $ShoesLibraries = (Resolve-Path -LiteralPath $ShoesLibraries).Path
    $provenance = Get-Content -Raw -LiteralPath (Join-Path $ShoesLibraries 'manifest.json') | ConvertFrom-Json -AsHashtable
    if ($provenance['revision'] -ne 'cce97a2c4c1c3bd0a07807f37a7caf3876c79110' -or $provenance['version'] -ne '0.3.7') {
        throw 'The Android Shoes bundle does not target the approved revision and version.'
    }
    $shoesHashes = $provenance['sha256']
    if (-not $shoesHashes) { throw 'Missing Android Shoes bundle digests.' }
}

$stage = $null
$signingStage = $null
Push-Location $repo
try {
    New-Item -ItemType Directory -Force -Path $work, $outputDirectory | Out-Null
    $stage = New-BuildStage $work 'package-'
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
        $entry = "lib/$platform/libvpn_android.so"
        $destination = Join-Path $stage $entry
        New-Item -ItemType Directory -Force -Path (Split-Path -Parent $destination) | Out-Null
        Copy-Item -LiteralPath (Join-Path $repo "target/$target/release/libvpn_android.so") -Destination $destination
        Invoke-Step $strip @('--strip-debug', $destination)
        $apkEntries += $entry
        if ($shoesHashes) {
            $engineEntry = "lib/$platform/libshoes.so"
            $engine = Resolve-Tool (Join-Path $ShoesLibraries "$platform/libshoes.so")
            if ((Get-FileHash -Algorithm SHA256 -LiteralPath $engine).Hash -ne $shoesHashes[$platform]) {
                throw "Android Shoes digest mismatch for $platform."
            }
            Copy-Item -LiteralPath $engine -Destination (Join-Path $stage $engineEntry)
            $apkEntries += $engineEntry
        }
    }

    Invoke-Step $aapt2 @('compile', '--dir', (Join-Path $package 'res'), '-o', $resources)
    Invoke-Step $aapt2 @('link', '-I', $androidJar, '--manifest', $manifest, '--debug-mode',
        '--java', $generatedSources, '-o', $unsigned, $resources)
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
    if (-not (Test-Path -LiteralPath $keystore -PathType Leaf)) {
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
    $signingStage = New-BuildStage $outputDirectory '.vpn-client-sign-'
    $signed = Join-Path $signingStage 'signed.apk'
    Invoke-Step $java @('-jar', $signer, 'sign', '--ks', $keystore, '--ks-key-alias', 'androiddebugkey',
        '--ks-pass', 'pass:android', '--key-pass', 'pass:android', '--out', $signed, $aligned)
    $verify = @('-jar', $signer, 'verify', '--verbose')
    if (Test-Path -LiteralPath "$signed.idsig" -PathType Leaf) {
        $verify += @('--v4-signature-file', "$signed.idsig")
    }
    Invoke-Step $java ($verify + $signed)
    Publish-VerifiedApk $signed $absOutput
    Write-Output $absOutput
} finally {
    Pop-Location
    try {
        Remove-BuildStage $signingStage $outputDirectory
    } finally {
        Remove-BuildStage $stage $work
    }
}
