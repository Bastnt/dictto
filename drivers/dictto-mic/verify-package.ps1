#requires -Version 7.2
param([Parameter(Mandatory)][string]$Package)
$ErrorActionPreference = 'Stop'
$Package = (Resolve-Path -LiteralPath $Package).Path
$catalog = Join-Path $Package DicttoMic.cat
$cert = [System.Security.Cryptography.X509Certificates.X509Certificate2]::new(
    (Join-Path $Package DicttoMic-test.cer))
$cms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
$cms.Decode([IO.File]::ReadAllBytes($catalog))
$cms.CheckSignature($true)
if ($cms.SignerInfos.Count -ne 1 -or $cms.SignerInfos[0].Certificate.Thumbprint -ne $cert.Thumbprint) {
    throw 'Unexpected catalog signer'
}

# Windows hashes PE catalog members differently from plain files.
if (-not ('DicttoCatalog' -as [type])) {
    Add-Type @"
using System;
using System.IO;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class DicttoCatalog {
    [DllImport("wintrust", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern bool CryptCATAdminAcquireContext2(out IntPtr admin, IntPtr subsystem, string hash, IntPtr policy, uint flags);
    [DllImport("wintrust", SetLastError=true)]
    static extern bool CryptCATAdminCalcHashFromFileHandle2(IntPtr admin, IntPtr file, ref uint size, byte[] digest, uint flags);
    [DllImport("wintrust")] static extern bool CryptCATAdminReleaseContext(IntPtr admin, uint flags);
    [DllImport("wintrust", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CryptCATOpen(string path, uint flags, IntPtr provider, uint version, uint encoding);
    [DllImport("wintrust", CharSet=CharSet.Unicode, SetLastError=true)]
    static extern IntPtr CryptCATGetMemberInfo(IntPtr cat, string tag);
    [DllImport("wintrust")] static extern bool CryptCATClose(IntPtr cat);
    public static void VerifyMember(string catPath, string filePath) {
        IntPtr admin;
        if (!CryptCATAdminAcquireContext2(out admin, IntPtr.Zero, "SHA256", IntPtr.Zero, 0))
            throw new Win32Exception();
        try {
            using (var file = File.OpenRead(filePath)) {
                uint size=32;
                var digest=new byte[size];
                if (!CryptCATAdminCalcHashFromFileHandle2(admin, file.SafeFileHandle.DangerousGetHandle(), ref size, digest, 0))
                    throw new Win32Exception();
                IntPtr cat=CryptCATOpen(catPath, 4, IntPtr.Zero, 0, 0x10001);
                if (cat == IntPtr.Zero || cat == new IntPtr(-1)) throw new Win32Exception();
                try {
                    string tag=BitConverter.ToString(digest).Replace("-", "");
                    if (CryptCATGetMemberInfo(cat, tag) == IntPtr.Zero)
                        throw new InvalidDataException("Missing or modified catalog member: " + filePath);
                } finally { CryptCATClose(cat); }
            }
        } finally { CryptCATAdminReleaseContext(admin, 0); }
    }
}
"@
}
foreach ($name in 'DicttoMic.inf','DicttoMic.sys') {
    [DicttoCatalog]::VerifyMember($catalog, (Join-Path $Package $name))
}
# The PE catalog digest omits the WIN_CERTIFICATE data. Verify its CMS separately.
$reader = [IO.BinaryReader]::new([IO.File]::OpenRead((Join-Path $Package DicttoMic.sys)))
try {
    $reader.BaseStream.Position = 0x3c
    $peOffset = $reader.ReadUInt32()
    $reader.BaseStream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x4550) { throw 'Invalid PE signature' }
    $reader.BaseStream.Position = $peOffset + 24
    if ($reader.ReadUInt16() -ne 0x20b) { throw 'Expected an x64 PE32+ image' }
    $reader.BaseStream.Position = $peOffset + 24 + 112 + 4 * 8
    $signatureOffset = $reader.ReadUInt32()
    $reader.BaseStream.Position = $signatureOffset
    $size = $reader.ReadUInt32()
    if ($size -lt 8 -or $size -gt $reader.BaseStream.Length - $signatureOffset -or
        $reader.ReadUInt16() -ne 0x200 -or $reader.ReadUInt16() -ne 2) { throw 'Invalid PE certificate' }
    $embeddedCms = [System.Security.Cryptography.Pkcs.SignedCms]::new()
    $embeddedCms.Decode($reader.ReadBytes($size - 8))
    $embeddedCms.CheckSignature($true)
    if ($embeddedCms.SignerInfos.Count -ne 1 -or $embeddedCms.SignerInfos[0].Certificate.Thumbprint -ne $cert.Thumbprint) {
        throw 'Unexpected embedded certificate'
    }
} finally { $reader.Dispose() }
$embedded = Get-AuthenticodeSignature -LiteralPath (Join-Path $Package DicttoMic.sys)
if ($embedded.SignerCertificate.Thumbprint -ne $cert.Thumbprint -or
    $embedded.Status -in 'HashMismatch', 'NotSigned') { throw 'Unexpected SYS signature' }
Write-Output "Verified SHA-256 catalog signature and both members. Signer $($cert.Thumbprint)"
