#requires -Version 7.2
param([string]$KitCache = (Join-Path $env:LOCALAPPDATA 'Dictto/DriverKit'))
$ErrorActionPreference = 'Stop'
$version = '10.0.28000.2526'
$sdk = '10.0.28000.0'
$project = Join-Path $PSScriptRoot 'DicttoMic.vcxproj'
$output = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../../.artifacts/dictto-mic'))
New-Item -ItemType Directory -Force $KitCache, $output | Out-Null
$KitCache = (Resolve-Path -LiteralPath $KitCache).Path

function Unpack-Pinned($Name, $Url, $Digest) {
    $archive = Join-Path $KitCache "$Name.nupkg"
    $folder = Join-Path $KitCache $Name
    if (-not (Test-Path -LiteralPath $archive)) {
        Invoke-WebRequest $Url -OutFile "$archive.part"
        if ((Get-FileHash "$archive.part").Hash -ne $Digest) { throw "Invalid download: $Name" }
        Move-Item -LiteralPath "$archive.part" -Destination $archive
    }
    if ((Get-FileHash $archive).Hash -ne $Digest) { throw "Invalid cached download: $Name" }
    if (-not (Test-Path -LiteralPath $folder)) {
        $stage = "$folder-$([guid]::NewGuid())"
        [IO.Compression.ZipFile]::ExtractToDirectory($archive, $stage)
        Move-Item -LiteralPath $stage -Destination $folder
    }
    return $folder
}
$pins = [ordered]@{
    'Microsoft.Windows.WDK.x64' = '63C939FB5A79295BF40E941DB592681272219B04EDFF095FE2F3D123E5579A90'
    'Microsoft.Windows.SDK.cpp.x64' = 'A9CAE2A8C5DA7F5DC5838AE6A76D06D0D2E2FDC3D8CFC69CA6C184E4B9193A00'
    'Microsoft.Windows.SDK.cpp' = 'BE1B419491607EAE6F7C57844EBAB39FACE9643C51E2AF1D9176A3BA0D0B23FC'
}
$imports = foreach ($id in $pins.Keys) {
    $slug = $id.ToLowerInvariant()
    $folder = Unpack-Pinned "$id.$version" "https://api.nuget.org/v3-flatcontainer/$slug/$version/$slug.$version.nupkg" $pins[$id]
    $props = [System.Security.SecurityElement]::Escape((Join-Path $folder "build/native/$id.props"))
    "<Import Project=""$props"" />"
}

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
$vs = & $vswhere -latest -products '*' -version '[18.0,19.0)' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vs) { throw 'Visual Studio 2026 with Desktop development with C++ is required' }
$vc = Join-Path $vs 'MSBuild/Microsoft/VC/v180'
if (-not (Test-Path "$vc/Platforms/x64/PlatformToolsets/WindowsKernelModeDriver10.0/Toolset.props")) {
    # Overlay the official VS WDK targets privately when only the C++ workload is installed.
    $hash = 'fc94cb3a77269a0c5374ee08292cb6b5804af86da04112763566f666daccf15b'
    $url = "https://download.visualstudio.microsoft.com/download/pr/bb2a0130-3d29-484b-a4a0-eea5313ed282/$hash/payload.vsix"
    $overlay = Unpack-Pinned 'wdk-build-targets-18.9' $url $hash
    $staged = Join-Path $KitCache 'v180'
    New-Item -ItemType Directory -Force $staged | Out-Null
    Copy-Item "$vc/*" $staged -Recurse -Force
    Copy-Item "$overlay/Contents/MSBuild/Microsoft/VC/v180/*" $staged -Recurse -Force
    $vc = $staged
}
$kitProps = Join-Path $KitCache 'NuGetKit.props'
"<Project>$imports</Project>" | Set-Content -LiteralPath $kitProps -Encoding utf8
$wdk = Join-Path $KitCache "Microsoft.Windows.WDK.x64.$version/c"
$sdkBin = Join-Path $KitCache "Microsoft.Windows.SDK.cpp.$version/c/bin/$sdk/x64"
$stage = Join-Path $output ([guid]::NewGuid().ToString())
$package = Join-Path $stage 'package'
New-Item -ItemType Directory -Force $package | Out-Null
$oldPath = $env:PATH
try {
    $env:PATH = "$wdk/bin/$sdk/x64;$sdkBin;$env:PATH"
    $msbuild = Join-Path $vs 'MSBuild/Current/Bin/amd64/MSBuild.exe'
    & $msbuild $project /nologo /v:minimal /p:Configuration=Release /p:Platform=x64 "/p:VCTargetsPath=$vc/" `
        "/p:DirectoryBuildPropsPath=$kitProps" "/p:WindowsTargetPlatformVersion=$sdk" `
        "/p:IntDir=$stage/obj/" "/p:OutDir=$stage/bin/" /p:SignMode=Off /p:EnableInf2cat=false `
        /p:SpectreMitigation=false /p:RunCodeAnalysis=false /p:SkipPackageVerification=true
    if ($LASTEXITCODE) { throw "MSBuild failed ($LASTEXITCODE)" }
    foreach ($name in 'DicttoMic.sys','DicttoMic.inf') { Copy-Item "$stage/bin/$name" $package }
    & "$wdk/tools/$sdk/x64/infverif.exe" /v /h "$package/DicttoMic.inf"
    if ($LASTEXITCODE) { throw "INF verification failed ($LASTEXITCODE)" }

    $subject = 'CN=Dictto Microphone Development Only'
    $cert = Get-ChildItem Cert:/CurrentUser/My -CodeSigningCert |
        Where-Object { $_.Subject -eq $subject -and $_.HasPrivateKey -and $_.NotAfter -gt (Get-Date) } |
        Sort-Object NotAfter -Descending | Select-Object -First 1
    if (-not $cert) {
        $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject -KeyExportPolicy NonExportable `
            -HashAlgorithm SHA256 -KeyAlgorithm RSA -KeyLength 3072 -CertStoreLocation Cert:/CurrentUser/My `
            -NotAfter (Get-Date).AddYears(1)
    }
    & "$sdkBin/signtool.exe" sign /q /s My /sha1 $cert.Thumbprint /fd SHA256 "$package/DicttoMic.sys"
    if ($LASTEXITCODE) { throw 'SYS signing failed' }
    & "$wdk/bin/$sdk/x86/Inf2Cat.exe" "/driver:$package" /os:10_NI_X64,10_GE_X64 /uselocaltime
    if ($LASTEXITCODE) { throw 'Catalog generation failed' }
    & "$sdkBin/signtool.exe" sign /q /s My /sha1 $cert.Thumbprint /fd SHA256 "$package/DicttoMic.cat"
    if ($LASTEXITCODE) { throw 'Catalog signing failed' }
    Export-Certificate -Cert $cert -FilePath "$package/DicttoMic-test.cer" | Out-Null
    & "$PSScriptRoot/verify-package.ps1" $package

    Copy-Item "$wdk/tools/$sdk/x64/devgen.exe" $package
    Copy-Item "$PSScriptRoot/install-dev.ps1","$PSScriptRoot/verify-package.ps1","$PSScriptRoot/tests/verify_stream.py","$PSScriptRoot/BUILDING.md" $package
    $licenses = New-Item -ItemType Directory (Join-Path $package 'licenses')
    Copy-Item "$PSScriptRoot/sysvad/LICENSE" (Join-Path $licenses 'sysvad-LICENSE.txt')
    Copy-Item "$PSScriptRoot/sysvad/UPSTREAM.txt" (Join-Path $licenses 'sysvad-UPSTREAM.txt')
    Copy-Item "$PSScriptRoot/../../LICENSE" (Join-Path $licenses 'Dictto-LICENSE.txt')
    $zip = Join-Path $stage 'DicttoMic-x64-test.zip'
    Compress-Archive -Path "$package/*" -DestinationPath $zip
    Write-Output "Package: $zip"
    Get-FileHash $zip | Format-List Algorithm,Hash
} finally {
    $env:PATH = $oldPath
}

