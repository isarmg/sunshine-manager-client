#Requires -RunAsAdministrator
$ErrorActionPreference = 'Stop'
$binDir = Join-Path $env:ProgramFiles 'Xscc'
$target = Join-Path $binDir 'xscc.exe'
if ((Get-Item -LiteralPath $binDir).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected installation link' }
$binary = Get-Item -LiteralPath $target -Force
if ($binary.PSIsContainer -or ($binary.Attributes -band [IO.FileAttributes]::ReparsePoint)) { throw 'Unexpected client executable' }
$stateDir = Join-Path $env:ProgramData 'Xscc'
$expectedCommand = '"' + $target + '" --windows-service --state "' + $stateDir + '"'
$registered = Get-CimInstance Win32_Service -Filter "Name='Xscc'"
if (-not $registered -or $registered.PathName -ne $expectedCommand -or $registered.StartName -ne 'LocalSystem') { throw 'Service registration does not match this installation' }
& $target service stop --timeout 30s --non-interactive
if ($LASTEXITCODE -ne 0) { throw 'Client service did not stop cleanly' }
(Get-Service Xscc).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(30))
& sc.exe delete Xscc
if ($LASTEXITCODE -ne 0) { throw 'Service deletion failed' }
Remove-Item -LiteralPath $target
# Nonrecursive: refuse to delete unrelated files.
[IO.Directory]::Delete($binDir,$false)
Write-Host 'Client executable/service removed. ProgramData\Xscc retained for recovery and deduplication. Revoke the device in Manager.'
