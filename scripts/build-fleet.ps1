param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
if (-not $SkipBuild) {
    Push-Location $projectRoot
    try {
        & pnpm --filter '@dictto/desktop' tauri build --no-bundle
        if ($LASTEXITCODE -ne 0) { throw 'Fleet release build failed' }
    } finally { Pop-Location }
}
$source = Join-Path $projectRoot 'apps/desktop/src-tauri/target/release/dictto.exe'
$destination = Join-Path $env:LOCALAPPDATA 'Programs/Dictto-Fleet'
$executable = Join-Path $destination 'dictto.exe'
New-Item -ItemType Directory -Path $destination -Force | Out-Null
Get-Process dictto -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $executable } | Stop-Process
if (Test-Path -LiteralPath $executable) {
    $backup = Join-Path $projectRoot ('build/dictto-fleet-' + (Get-Date -Format 'yyyyMMdd-HHmmss') + '.exe')
    New-Item -ItemType Directory -Path (Split-Path -Parent $backup) -Force | Out-Null
    Copy-Item -LiteralPath $executable -Destination $backup
}
Copy-Item -LiteralPath $source -Destination $executable -Force
if ((Get-FileHash -LiteralPath $source).Hash -ne (Get-FileHash -LiteralPath $executable).Hash) {
    throw 'Installed Fleet executable hash mismatch'
}
Write-Output "Installed Fleet build: $executable"
