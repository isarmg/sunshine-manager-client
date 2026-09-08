#Requires -RunAsAdministrator
param([Parameter(Mandatory=$true)][string]$Binary,[Parameter(Mandatory=$true)][string]$Bootstrap)
$ErrorActionPreference = 'Stop'
if ([System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture -ne 'X64') { throw 'Windows x86_64 required' }
if ([int](Get-ItemPropertyValue 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' 'CurrentBuildNumber') -lt 22000) { throw 'Windows 11 or newer required' }
foreach ($source in @($Binary,$Bootstrap)) {
  if ($source -notmatch '^[A-Za-z]:\\') { throw 'Absolute local drive paths required' }
  $item = Get-Item -LiteralPath $source
  if ($item.PSIsContainer -or ($item.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Regular source files required' }
}
$identity = & $Binary --version
if ($LASTEXITCODE -ne 0 -or $identity -notlike 'sunshine-client *') { throw 'Expected a verified Sunshine Client binary' }
$binDir = Join-Path $env:ProgramFiles 'SunshineClient'
$stateDir = Join-Path $env:ProgramData 'SunshineClient'
if ((Test-Path -LiteralPath $binDir) -or (Test-Path -LiteralPath $stateDir) -or (Get-Service SunshineClient -ErrorAction SilentlyContinue)) {
  throw 'Refusing to overwrite installation/state. Upgrade and restore belong to sarmg-upgrade.'
}
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
$null = New-Item -ItemType Directory -Path $binDir
Protect-LocalPath $binDir $true
$target = Join-Path $binDir 'sunshine-client.exe'
Copy-Item -LiteralPath $Binary -Destination $target
& $target init --state $stateDir --bootstrap $Bootstrap
if ($LASTEXITCODE -ne 0) { throw 'Protected bootstrap import failed; retained partial installation for inspection.' }
# Drop the installing user's individual ACE before running as LocalSystem.
Get-ChildItem -LiteralPath $stateDir -Recurse -Force | ForEach-Object {
  if ($_.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected reparse point' }
  Protect-LocalPath $_.FullName $_.PSIsContainer
}
Protect-LocalPath $stateDir $true
$command = '"' + $target + '" service --state "' + $stateDir + '"'
$null = New-Service -Name SunshineClient -DisplayName 'Sunshine management Client' -BinaryPathName $command -StartupType Manual -Description 'Independent management only; no video forwarding or general remote control.'
Start-Service SunshineClient
(Get-Service SunshineClient).WaitForStatus('Running',[TimeSpan]::FromSeconds(30))
Write-Host 'Client installed. Verify registration, then remove the original protected bootstrap. No Sunshine process or firewall rule was changed.'
