#requires -Version 7.2
param([switch]$Elevated)
$ErrorActionPreference = 'Stop'
try {
    $admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
    if (-not $admin) {
        if ($Elevated) { throw 'Administrator access was not granted.' }
        $process = Start-Process (Join-Path $PSHOME 'pwsh.exe') -Verb RunAs -Wait -PassThru -ArgumentList @('-NoProfile', '-File', ('"' + $PSCommandPath + '"'), '-Elevated')
        exit $process.ExitCode
    }
    & "$PSScriptRoot/verify-package.ps1" -Package $PSScriptRoot
    $devgen = Join-Path $PSScriptRoot 'devgen.exe'
    if (-not (Test-Path -LiteralPath $devgen)) {
        $devgen = Get-ChildItem "$env:LOCALAPPDATA/Dictto/DriverKit" -Filter devgen.exe -Recurse -ErrorAction SilentlyContinue |
            Where-Object { $_.Directory.Name -eq 'x64' } | Select-Object -First 1 -ExpandProperty FullName
    }
    if (-not $devgen) { throw 'Missing x64 devgen.exe. Copy it from the Windows WDK into this package folder.' }
    function Invoke-Checked([string]$Exe, [string[]]$Arguments) {
        & $Exe @Arguments
        if ($Exe -eq 'pnputil.exe' -and $Arguments[0] -eq '/add-driver' -and $LASTEXITCODE -eq 259) { return }
        if ($LASTEXITCODE -notin 0,3010) { throw "$Exe failed with exit code $LASTEXITCODE" }
    }
    Invoke-Checked certutil.exe @('-addstore', 'Root', "$PSScriptRoot/DicttoMic-test.cer")
    Invoke-Checked certutil.exe @('-addstore', 'TrustedPublisher', "$PSScriptRoot/DicttoMic-test.cer")
    Invoke-Checked pnputil.exe @('/add-driver', "$PSScriptRoot/DicttoMic.inf", '/install')
    $existing = Get-PnpDevice -PresentOnly | Where-Object {
        $_.InstanceId -like 'ROOT\*' -and
        ((Get-PnpDeviceProperty -InstanceId $_.InstanceId -KeyName DEVPKEY_Device_HardwareIds -ErrorAction SilentlyContinue).Data -contains 'ROOT\DICTTOMIC')
    }
    if (-not $existing) { Invoke-Checked $devgen @('/add', '/bus', 'ROOT', '/hardwareid', 'ROOT\DICTTOMIC') }
    Invoke-Checked pnputil.exe @('/add-driver', "$PSScriptRoot/DicttoMic.inf", '/install')
    Write-Host 'Installation commands completed. Windows must already allow test-signed drivers. Boot security settings were not changed.'
    Get-PnpDevice -PresentOnly | Where-Object FriendlyName -like '*Dictto*' | Format-Table Status, FriendlyName, InstanceId
} catch {
    Write-Host $_ -ForegroundColor Red
    if ($Elevated) { Read-Host 'Press Enter to close' | Out-Null }
    exit 1
}
if ($Elevated) { Read-Host 'Press Enter to close' | Out-Null }

