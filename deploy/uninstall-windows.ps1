#Requires -RunAsAdministrator
$ErrorActionPreference = 'Stop'
$binDir = Join-Path $env:ProgramFiles 'SunshineClient'
$target = Join-Path $binDir 'sunshine-client.exe'
if ((Get-Item -LiteralPath $binDir).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'Unexpected installation link' }
Stop-Service SunshineClient
(Get-Service SunshineClient).WaitForStatus('Stopped',[TimeSpan]::FromSeconds(30))
& sc.exe delete SunshineClient
if ($LASTEXITCODE -ne 0) { throw 'Service deletion failed' }
Remove-Item -LiteralPath $target
# Nonrecursive: refuse to delete unrelated files.
[IO.Directory]::Delete($binDir,$false)
Write-Host 'Client executable/service removed. ProgramData\SunshineClient retained for recovery and deduplication. Revoke the device in Manager.'
