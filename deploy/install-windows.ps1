#Requires -RunAsAdministrator
param([Parameter(Mandatory=$true)][string]$Binary,[string]$Bootstrap)
$ErrorActionPreference = 'Stop'
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'Windows x86_64 required' }
if ([int](Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' 'CurrentBuildNumber') -lt 22000) { throw 'Windows 11 or newer required' }
$sources = @($Binary)
if ($Bootstrap) { $sources += $Bootstrap }
foreach ($source in $sources) {
  if ($source -notmatch '^[A-Za-z]:\\') { throw 'Absolute local drive paths required' }
  $item = Get-Item -LiteralPath $source
  if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Regular source files required' }
}
$identity = & $Binary --version
if ($LASTEXITCODE -ne 0 -or $identity -notlike 'sunshine-client *') { throw 'Expected a verified Sunshine Client binary' }
$binDir = Join-Path $env:ProgramFiles 'SunshineClient'
$stateDir = Join-Path $env:ProgramData 'SunshineClient'
function Protect-LocalPath([string]$Path,[bool]$Directory) {
  $acl = if ($Directory) { New-Object System.Security.AccessControl.DirectorySecurity } else { New-Object System.Security.AccessControl.FileSecurity }
  $acl.SetAccessRuleProtection($true,$false)
  $admin = New-Object System.Security.Principal.SecurityIdentifier 'S-1-5-32-544'
  $acl.SetOwner($admin)
  foreach ($sid in @('S-1-5-18','S-1-5-32-544')) {
    $identity = New-Object System.Security.Principal.SecurityIdentifier $sid
    $inherit = if ($Directory) { [System.Security.AccessControl.InheritanceFlags]'ContainerInherit,ObjectInherit' } else { [System.Security.AccessControl.InheritanceFlags]::None }
    $rule = New-Object System.Security.AccessControl.FileSystemAccessRule($identity,'FullControl',$inherit,'None','Allow')
    $acl.AddAccessRule($rule)
  }
  Set-Acl -LiteralPath $Path -AclObject $acl
}
$target = Join-Path $binDir 'sunshine-client.exe'
$command = '"' + $target + '" --windows-service --state "' + $stateDir + '"'
$existingService = Get-CimInstance Win32_Service -Filter "Name='SunshineClient'"
if ($existingService -and ($existingService.PathName -ne $command -or $existingService.StartName -ne 'LocalSystem')) { throw 'Same-name service points outside the managed Client installation.' }
foreach ($path in @($binDir,$stateDir,$target)) {
    if (Test-Path -LiteralPath $path) {
        $item = Get-Item -LiteralPath $path -Force
        if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Refusing a redirected installation path.' }
        $acl = Get-Acl -LiteralPath $path
        $owner = $acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
        if ($owner -notin @('S-1-5-18','S-1-5-32-544')) { throw 'Existing product path is not administrator owned.' }
    }
}
$madeBinary = $false
$madeState = $false
$madeService = $false
$hadTarget = Test-Path -LiteralPath $target -PathType Leaf
$wasRunning = $existingService -and $existingService.State -eq 'Running'
$backup = Join-Path $env:ProgramData ('SunshineClient-repair-' + [guid]::NewGuid())
$null = New-Item -ItemType Directory -Path $backup
Protect-LocalPath $backup $true
Copy-Item -LiteralPath $Binary -Destination (Join-Path $backup 'new.exe') -Force
if ($hadTarget) { Copy-Item -LiteralPath $target -Destination (Join-Path $backup 'previous.exe') -Force }
try {
    if ($existingService) { Stop-Service SunshineClient -Force -ErrorAction Stop }
    if (-not (Test-Path -LiteralPath $binDir)) {
        $null = New-Item -ItemType Directory -Path $binDir
        $madeBinary = $true
    }
    Protect-LocalPath $binDir $true
    $target = Join-Path $binDir 'sunshine-client.exe'
    Copy-Item -LiteralPath (Join-Path $backup 'new.exe') -Destination $target -Force
    if (-not (Test-Path -LiteralPath $stateDir)) {
        $null = New-Item -ItemType Directory -Path $stateDir
        $madeState = $true
        Protect-LocalPath $stateDir $true
    }
    if ($Bootstrap -and $madeState) {
      & $target init --state $stateDir --bootstrap $Bootstrap
      if ($LASTEXITCODE -ne 0) { throw 'Protected bootstrap import failed; installation will roll back.' }
    }
    # Drop the installing user's individual ACE before running as LocalSystem.
    if ($madeState) { Get-ChildItem -LiteralPath $stateDir -Recurse -Force | ForEach-Object {
      if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected reparse point' }
      Protect-LocalPath $_.FullName $_.PSIsContainer
    }
    Protect-LocalPath $stateDir $true
    }
    $command = '"' + $target + '" --windows-service --state "' + $stateDir + '"'
    if (-not $existingService) {
    $null = New-Service -Name SunshineClient -DisplayName 'Sunshine management Client' -BinaryPathName $command -StartupType Manual -Description 'Independent management only; no video forwarding or general remote control.'
    $madeService = $true
    }
    if ($wasRunning) { Start-Service SunshineClient }
    Remove-Item -LiteralPath $backup -Recurse -Force -ErrorAction SilentlyContinue
    Write-Host 'Client installed. Run the installed sunshine-client.exe setup --interactive. No Sunshine process or firewall rule was changed.'
} catch {
    $original = $_
    # Restore an overwritten program; remove only directories created here.
    # Existing identity and task journal are never rollback deletion targets.
    try {
        if ($hadTarget) { Copy-Item -LiteralPath (Join-Path $backup 'previous.exe') -Destination $target -Force }
        elseif (-not $madeBinary -and (Test-Path -LiteralPath $target)) { Remove-Item -LiteralPath $target -Force }
        if ($wasRunning) { Start-Service SunshineClient }
        if ($madeService) {
            & sc.exe delete SunshineClient
            if ($LASTEXITCODE -ne 0) { throw 'Could not roll back the new service registration' }
        }
        foreach ($created in @(@($stateDir,$madeState), @($binDir,$madeBinary))) {
            if ($created[1] -and (Test-Path -LiteralPath $created[0])) {
                $item = Get-Item -LiteralPath $created[0]
                if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Rollback target changed; retained for inspection' }
                Remove-Item -LiteralPath $created[0] -Recurse -Force
            }
        }
    } catch {
        Write-Warning ('Installation rollback requires inspection: ' + $_.Exception.Message)
    }
    Write-Warning "Previous program backup retained at $backup"
    throw $original
}
