# Dictto Microphone driver package

Dictto Microphone is an x64 Windows capture endpoint. The development package targets Windows 11 build 22621 or newer. It contains a kernel driver, its INF, a signed catalog and the public development certificate.

## Build and verify

Use PowerShell 7.2+ on Windows x64 with Visual Studio 2026 and its Desktop development with C++ workload. From this directory run:

```powershell
./build-dev.ps1
```

The script downloads pinned Microsoft SDK and WDK NuGet archives and verifies their SHA-256 hashes. If Visual Studio lacks the driver MSBuild targets, it overlays Microsoft's VS 18 targets in a private cache. The default cache is `%LOCALAPPDATA%\Dictto\DriverKit`. The output path and SHA-256 of `DicttoMic-x64-test.zip` are printed at the end. Each build has its own directory under the repository's `.artifacts\dictto-mic`. [Microsoft's WDK NuGet documentation](https://learn.microsoft.com/en-us/windows-hardware/drivers/install-the-wdk-using-nuget)

The script creates or reuses a non-exportable development key in the building user's Personal store, embeds a SHA-256 signature in the SYS, runs WDK InfVerif and Inf2Cat, and signs the catalog. To verify an extracted package independently run:

```powershell
./verify-package.ps1 -Package C:\driver-test\package
```

The verifier checks the catalog signature, the signed SHA-256 membership of both files, and the signer of the embedded SYS signature. Match the printed certificate thumbprint and ZIP digest against those supplied by the builder over a trusted channel.

## Install and test in a disposable VM

Snapshot a Windows x64 test VM that is already configured for test-signed kernel drivers. Extract the package and copy the WDK's x64 `devgen.exe` beside the included verifier and `verify_stream.py`. The default build cache contains DevGen at `%LOCALAPPDATA%\Dictto\DriverKit\Microsoft.Windows.WDK.x64.10.0.28000.2526\c\tools\10.0.28000.0\x64\devgen.exe`. The build does not install a device or add its signing certificate to trusted stores. [Microsoft test VM prerequisites](https://learn.microsoft.com/en-us/windows-hardware/drivers/install/installing-test-signed-driver-packages)

After verifying the package and its expected signer, from an **elevated PowerShell in that VM**:

```powershell
./verify-package.ps1 -Package .
certutil -addstore Root .\DicttoMic-test.cer
certutil -addstore TrustedPublisher .\DicttoMic-test.cer
pnputil /add-driver .\DicttoMic.inf
.\devgen.exe /add /bus ROOT /hardwareid 'ROOT\DICTTOMIC'
pnputil /enum-devices /class Media /connected
```

Keep the published INF name from PnPUtil and instance ID from DevGen. Run DevGen once per VM. Open an ordinary, non-elevated PowerShell in the interactive desktop session and run `python .\verify_stream.py`. This also checks the interactive user's write access to the feed device. The harness selects Dictto Microphone by name, sends three tones through the feed and checks the captured order. It requires standard Python 3 on Windows. A successful run prints all three recovered tones. The separate `--self-test` option exercises only the signal detector.

### Live Discord acceptance on the test VM

1. Record the current Windows default communications microphone. In Discord's Voice & Video settings, set the input device to **Default**. Leave the mute button off for the test.
2. In Dictto, enable **Send speech to Dictto Microphone**, join a private test call with a consenting listener, and trigger a speech action containing a distinctive sentence.
3. Ask the listener to repeat that sentence. Check the communications microphone returns to the original device and that the listener hears normal microphone input again.
4. Repeat the action and cancel it. Confirm that playback stops and the original communications microphone returns. If Discord retains its old input stream through device changes, record that result separately from the harness result. Neither the package checks nor the harness establish Discord's live routing behavior.

To undo the test, restore the snapshot. For a VM that must keep other changes, use `pnputil /remove-device "<instance ID>"`, then `pnputil /delete-driver <published oemNN.inf>`. Remove the matching development certificate from the VM's Root and TrustedPublisher stores.

## Release

Shipping to ordinary secured Windows installations requires the Hardware Dev Center signing path with the publisher's identity and the applicable compatibility testing. A successful local build verifies the package structure and signature; endpoint enumeration and PCM transfer require a driver load in a Windows test system. [Microsoft driver signing options](https://learn.microsoft.com/en-us/windows-hardware/drivers/dashboard/driver-signing-offerings)
