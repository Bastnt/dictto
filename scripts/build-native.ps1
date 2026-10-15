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
New-Item -ItemType Directory -Path $destination -Force | Out-Null
$executable = Join-Path $destination 'dictto-native.exe'
Get-Process dictto-native -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $executable } | Stop-Process
$buildDirectories = @('build', 'deps', 'examples', 'incremental')
foreach ($item in Get-ChildItem -LiteralPath $release -Force) {
    if ($item.PSIsContainer) {
        if ($item.Name.StartsWith('.') -or $item.Name -in $buildDirectories) { continue }
        Copy-Item -LiteralPath $item.FullName -Destination $destination -Recurse -Force
    } elseif ($item.Extension -in @('.dll', '.pri', '.ico') -or $item.Name -eq 'dictto-native.exe') {
        Copy-Item -LiteralPath $item.FullName -Destination $destination -Force
    }
}
$sourceHash = (Get-FileHash -LiteralPath (Join-Path $release 'dictto-native.exe')).Hash
$installedHash = (Get-FileHash -LiteralPath $executable).Hash
if ($sourceHash -ne $installedHash) { throw 'Installed native executable hash mismatch' }
$startMenu = Join-Path $env:APPDATA 'Microsoft/Windows/Start Menu/Programs'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut((Join-Path $startMenu 'Dictto Native.lnk'))
$shortcut.TargetPath = $executable
$shortcut.WorkingDirectory = $destination
$shortcut.IconLocation = Join-Path $destination 'icon.ico'
$shortcut.Save()
Write-Output "Installed native build: $executable"
Write-Output "SHA-256: $installedHash"
