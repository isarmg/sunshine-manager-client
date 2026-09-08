param([Parameter(Mandatory=$true)][string]$ClientExe, [Parameter(Mandatory=$true)][string]$Output, [Parameter(Mandatory=$true)][string]$Version)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSI version must be numeric' }
$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path $env:TEMP ('sunshine-msi-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
& dotnet build (Join-Path $root 'packaging/windows/SunshineClient.wixproj') -c Release "-p:ClientExe=$ClientExe" "-p:ProductVersion=$Version" "-p:BaseIntermediateOutputPath=$work\obj\" "-p:OutputPath=$Output\"
if ($LASTEXITCODE) { throw 'Installer compilation failed' }
