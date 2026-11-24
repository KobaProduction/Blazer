Set-StrictMode -Version Latest
$script:BlazerProduct = 'KobaProduction.Blazer.Windows'
$script:BlazerRegistry = 'SOFTWARE\KobaProduction\Blazer'
$script:BlazerUninstallRegistry = 'SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\KobaProduction.Blazer.Windows'

function Get-BlazerDirectory {
    Join-Path ([Environment]::GetFolderPath('ProgramFiles')) 'Blazer'
}

function Get-BlazerSetupStateDirectory {
    Join-Path ([Environment]::GetFolderPath('CommonApplicationData')) 'KobaProduction\Blazer'
}

function Assert-BlazerRegularPath {
    param([string]$Path, [string]$Root)
    $absolute = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    $boundary = [IO.Path]::GetFullPath($Root).TrimEnd('\')
    if ($absolute -ine $boundary -and -not $absolute.StartsWith($boundary + '\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'An installer path escaped its owned directory.'
    }
    for ($cursor = $absolute; $cursor.Length -ge $boundary.Length; $cursor = Split-Path -Parent $cursor) {
        if ((Test-Path -LiteralPath $cursor) -and
            ((Get-Item -LiteralPath $cursor -Force).Attributes -band [IO.FileAttributes]::ReparsePoint)) {
            throw 'Installer directories cannot contain reparse points.'
        }
        if ($cursor -ieq $boundary) { break }
    }
    $absolute
}

function Assert-BlazerFileName {
    param([string]$Name)
    $rootNames = @('blazer.exe','blazecore.exe','wintun.dll','uninstall.exe','LICENSE','LICENSE-NOTO-EMOJI',
        'LICENSE-BLAZECORE','THIRD_PARTY_NOTICES.md','blazer-installation.json','blazer-owned-files.json')
    if ($Name -cnotin $rootNames -and $Name -cnotmatch '^licenses/[A-Za-z0-9][A-Za-z0-9._-]*$') {
        throw "Unexpected owned payload filename: $Name"
    }
}

function Get-BlazerRegistration {
    Assert-BlazerRegistryPath $script:BlazerRegistry
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    try {
        $key = $base.OpenSubKey($script:BlazerRegistry)
        if (-not $key) { return $null }
        try {
            $result = @{}
            foreach ($name in $key.GetValueNames()) { $result[$name] = $key.GetValue($name) }
            $result
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}

function Set-BlazerRegistration {
    param([hashtable]$Values)
    Assert-BlazerRegistryPath $script:BlazerRegistry
    Set-BlazerProtectedRegistry $script:BlazerRegistry
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    try {
        $key = $base.CreateSubKey($script:BlazerRegistry)
        try {
            foreach ($name in $key.GetValueNames()) { $key.DeleteValue($name) }
            foreach ($name in $Values.Keys) {
                $kind = if ($name -eq 'Sequence') { 'DWord' } else { 'String' }
                $key.SetValue($name, $Values[$name], $kind)
            }
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}

function Remove-BlazerRegistration {
    Remove-BlazerShortcut
    Assert-BlazerRegistryPath $script:BlazerRegistry
    Assert-BlazerRegistryPath $script:BlazerUninstallRegistry
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    try {
        $base.DeleteSubKeyTree($script:BlazerRegistry, $false)
        $base.DeleteSubKeyTree($script:BlazerUninstallRegistry, $false)
    } finally { $base.Dispose() }
}

function Get-BlazerShortcutPath {
    Join-Path ([Environment]::GetFolderPath('CommonPrograms')) 'Blazer.lnk'
}

function Assert-BlazerShortcut {
    $path = Get-BlazerShortcutPath
    if (-not [IO.File]::Exists($path)) { return }
    [void](Assert-BlazerRegularPath $path (Split-Path -Parent $path))
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut($path)
        if ($shortcut.TargetPath -ine (Join-Path (Get-BlazerDirectory) 'blazer.exe')) {
            throw 'The Blazer Start Menu entry belongs to another application.'
        }
    } finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
}

function Set-BlazerShortcut {
    Assert-BlazerShortcut
    $shell = New-Object -ComObject WScript.Shell
    try {
        $shortcut = $shell.CreateShortcut((Get-BlazerShortcutPath))
        $shortcut.TargetPath = Join-Path (Get-BlazerDirectory) 'blazer.exe'
        $shortcut.WorkingDirectory = Get-BlazerDirectory
        $shortcut.IconLocation = $shortcut.TargetPath + ',0'
        $shortcut.Save()
    } finally { [void][Runtime.InteropServices.Marshal]::ReleaseComObject($shell) }
}

function Remove-BlazerShortcut {
    Assert-BlazerShortcut
    $path = Get-BlazerShortcutPath
    if ([IO.File]::Exists($path)) { [IO.File]::Delete($path) }
}

function Set-BlazerUninstallRegistration {
    param([string]$Directory, [string]$Version)
    Assert-BlazerRegistryPath $script:BlazerUninstallRegistry
    Set-BlazerProtectedRegistry $script:BlazerUninstallRegistry
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    try {
        $key = $base.CreateSubKey($script:BlazerUninstallRegistry)
        try {
            $values = @{ DisplayName='Blazer'; DisplayVersion=$Version; Publisher='KobaProduction';
                InstallLocation=$Directory; DisplayIcon=(Join-Path $Directory 'blazer.exe');
                UninstallString=('"' + (Join-Path $Directory 'uninstall.exe') + '"');
                URLInfoAbout='https://github.com/KobaProduction/Blazer' }
            foreach ($name in $values.Keys) { $key.SetValue($name, $values[$name], 'String') }
            $key.SetValue('NoModify', 1, 'DWord'); $key.SetValue('NoRepair', 1, 'DWord')
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}

function Assert-BlazerRegistryPath {
    param([string]$Path)
    if ($Path -cnotin @($script:BlazerRegistry,$script:BlazerUninstallRegistry)) { throw 'Unexpected product registry namespace.' }
    $cursor = $Path
    while ($cursor -ine 'SOFTWARE') {
        $registryPath = 'Registry::HKEY_LOCAL_MACHINE\' + $cursor
        if (Test-Path -LiteralPath $registryPath) {
            # Windows PowerShell 5.1 misresolves registry Get-Acl -LiteralPath.
            # Escape the fixed path rather than enabling wildcard selection.
            $acl = Get-Acl -Path ([Management.Automation.WildcardPattern]::Escape($registryPath))
            Assert-BlazerRegistrySecurity $acl ($cursor -ceq $Path)
        }
        $cursor = $cursor.Substring(0,$cursor.LastIndexOf('\'))
    }
}

function Assert-BlazerRegistrySecurity {
    param([Security.AccessControl.RegistrySecurity]$Acl, [bool]$Private)
    $trusted = @('S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    $sidType = [Security.Principal.SecurityIdentifier]
    if ($Acl.GetOwner($sidType).Value -notin $trusted) { throw 'An installation registry key has an unprivileged owner.' }
    if ($Private -and -not $Acl.AreAccessRulesProtected) { throw 'An owned registry key must have a protected DACL.' }
    $rights = if ($Private) { 'SetValue,CreateSubKey,Delete,ChangePermissions,TakeOwnership' }
        else { 'ChangePermissions,TakeOwnership' }
    $generic = if ($Private) { 0x50000000 } else { 0x10000000 }
    $mask = [int][Security.AccessControl.RegistryRights]$rights -bor $generic
    foreach ($rule in $Acl.GetAccessRules($true,$true,$sidType)) {
        if ($rule.AccessControlType -eq 'Allow' -and ($rule.RegistryRights -band $mask) -and
            -not ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and
            $rule.IdentityReference.Value -notin $trusted) { throw 'An installation registry key has unsafe access-control permissions.' }
    }
}

function Set-BlazerProtectedRegistry {
    param([string]$Path)
    if ($Path -cnotin @($script:BlazerRegistry,$script:BlazerUninstallRegistry)) { throw 'Unexpected product registry namespace.' }
    $acl = [Security.AccessControl.RegistrySecurity]::new()
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    $acl.SetAccessRuleProtection($true,$false)
    foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.RegistryAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', 'ContainerInherit', 'None', 'Allow'))
    }
    $acl.AddAccessRule([Security.AccessControl.RegistryAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'), 'ReadKey', 'ContainerInherit', 'None', 'Allow'))
    $base = [Microsoft.Win32.RegistryKey]::OpenBaseKey('LocalMachine', 'Registry64')
    try {
        $key = $base.CreateSubKey($Path, [Microsoft.Win32.RegistryKeyPermissionCheck]::ReadWriteSubTree, $acl)
        try {
            # CreateSubKey can return a concurrently precreated key. Never adopt
            # it by overwriting its owner/DACL before checking the actual handle.
            Assert-BlazerRegistrySecurity $key.GetAccessControl() $true
            $key.SetAccessControl($acl)
        } finally { $key.Dispose() }
    } finally { $base.Dispose() }
}

function Set-BlazerProtectedDirectory {
    param([string]$Directory)
    [IO.Directory]::CreateDirectory($Directory) | Out-Null
    [void](Assert-BlazerRegularPath $Directory (Split-Path -Parent $Directory))
    $acl = [Security.AccessControl.DirectorySecurity]::new()
    $admins = [Security.Principal.SecurityIdentifier]::new('S-1-5-32-544')
    $acl.SetOwner($admins)
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
        $rule = [Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', 'ContainerInherit,ObjectInherit', 'None', 'Allow')
        $acl.AddAccessRule($rule)
    }
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'), 'ReadAndExecute', 'ContainerInherit,ObjectInherit', 'None', 'Allow'))
    Set-Acl -LiteralPath $Directory -AclObject $acl
}

function Assert-BlazerProtectedPath {
    param([string]$Path)
    $trusted = @('S-1-5-18','S-1-5-32-544','S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')
    $acl = Get-Acl -LiteralPath $Path
    $sidType = [Security.Principal.SecurityIdentifier]
    if ($acl.GetOwner($sidType).Value -notin $trusted) { throw 'An installer-owned path has an unprivileged owner.' }
    $mask = [int][Security.AccessControl.FileSystemRights]'Write,Delete,DeleteSubdirectoriesAndFiles,ChangePermissions,TakeOwnership' -bor 0x50000000
    foreach ($rule in $acl.GetAccessRules($true, $true, $sidType)) {
        if ($rule.AccessControlType -eq 'Allow' -and ($rule.FileSystemRights -band $mask) -and
            -not ($rule.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and
            $rule.IdentityReference.Value -notin $trusted) { throw 'An installer-owned path is writable by an unprivileged principal.' }
    }
}

function Write-BlazerJson {
    param([string]$Path, $Value)
    $temporary = Join-Path (Split-Path -Parent $Path) ([Guid]::NewGuid().ToString('N') + '.tmp')
    $bytes = [Text.UTF8Encoding]::new($false).GetBytes(($Value | ConvertTo-Json -Depth 12))
    try {
        $stream = [IO.FileStream]::new($temporary, 'CreateNew', 'Write', 'None', 4096, 'WriteThrough')
        try { $stream.Write($bytes, 0, $bytes.Length); $stream.Flush($true) }
        finally { $stream.Dispose() }
        if ([IO.File]::Exists($Path)) { [IO.File]::Replace($temporary, $Path, [NullString]::Value) }
        else { [IO.File]::Move($temporary, $Path) }
    } finally { if ([IO.File]::Exists($temporary)) { [IO.File]::Delete($temporary) } }
}

function Set-BlazerProtectedFile {
    param([string]$Path)
    $acl = [Security.AccessControl.FileSecurity]::new()
    $acl.SetOwner([Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))
    $acl.SetAccessRuleProtection($true, $false)
    foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
        $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
            [Security.Principal.SecurityIdentifier]::new($sid), 'FullControl', 'Allow'))
    }
    $acl.AddAccessRule([Security.AccessControl.FileSystemAccessRule]::new(
        [Security.Principal.SecurityIdentifier]::new('S-1-5-32-545'), 'ReadAndExecute', 'Allow'))
    Set-Acl -LiteralPath $Path -AclObject $acl
}
