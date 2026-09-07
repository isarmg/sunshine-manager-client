param([Parameter(Mandatory=$true)][string]$ClientExe, [Parameter(Mandatory=$true)][string]$Output, [Parameter(Mandatory=$true)][string]$Version)
$ErrorActionPreference = 'Stop'
if ($Version -notmatch '^\d+\.\d+\.\d+$') { throw 'MSI version must be numeric' }
$root = Split-Path -Parent $PSScriptRoot
$work = Join-Path $env:TEMP ('sunshine-msi-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $work | Out-Null
$tray = Join-Path $work 'sunshine-client-tray.exe'
$compiler = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
& $compiler /nologo /target:winexe /platform:x64 /optimize+ "/out:$tray" /reference:System.Windows.Forms.dll /reference:System.Drawing.dll /reference:System.ServiceProcess.dll /reference:System.Web.Extensions.dll (Join-Path $root 'packaging/windows/Tray.cs')
if ($LASTEXITCODE) { throw 'Tray compilation failed' }
& dotnet build (Join-Path $root 'packaging/windows/SunshineClient.wixproj') -c Release "-p:ClientExe=$ClientExe" "-p:TrayExe=$tray" "-p:ProductVersion=$Version" "-p:BaseIntermediateOutputPath=$work\obj\" "-p:OutputPath=$Output\"
if ($LASTEXITCODE) { throw 'Installer compilation failed' }
