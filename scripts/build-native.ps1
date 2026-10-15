param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$manifest = Join-Path $projectRoot 'apps/native/Cargo.toml'
if (-not $SkipBuild) {
    & cargo build --release --manifest-path $manifest
    if ($LASTEXITCODE -ne 0) { throw 'Native release build failed' }
}
$release = (Resolve-Path (Join-Path $projectRoot 'apps/native/target/release')).Path
$destination = Join-Path $env:LOCALAPPDATA 'Programs/Dictto-Native'
$executable = Join-Path $destination 'dictto-native.exe'
Get-Process dictto-native -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $executable } | Stop-Process
if (Test-Path -LiteralPath (Join-Path $destination 'Microsoft.ui.xaml.dll')) {
    $programsRoot = [IO.Path]::GetFullPath((Join-Path $env:LOCALAPPDATA 'Programs'))
    $resolvedDestination = (Resolve-Path -LiteralPath $destination).Path
    if ($resolvedDestination -ne (Join-Path $programsRoot 'Dictto-Native')) { throw 'Unexpected native installation path' }
    $backup = Join-Path $programsRoot ('Dictto-Native-WinUI-backup-' + (Get-Date -Format 'yyyyMMdd-HHmmss'))
    Move-Item -LiteralPath $resolvedDestination -Destination $backup
    Write-Output "Previous WinUI install retained at: $backup"
}
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Copy-Item -LiteralPath (Join-Path $release 'dictto-native.exe') -Destination $executable -Force
$sourceHash = (Get-FileHash -LiteralPath (Join-Path $release 'dictto-native.exe')).Hash
$installedHash = (Get-FileHash -LiteralPath $executable).Hash
if ($sourceHash -ne $installedHash) { throw 'Installed native executable hash mismatch' }
$startMenu = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startMenu 'Dictto Native.lnk'))
$shortcut.TargetPath = $executable
$shortcut.WorkingDirectory = $destination
$shortcut.IconLocation = "$executable,0"
$shortcut.Save()
Write-Output "Installed native build: $executable"
Write-Output "SHA-256: $installedHash"
