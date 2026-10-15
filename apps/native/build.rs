fn main() {
    winresource::WindowsResource::new()
        .set_icon("../desktop/src-tauri/icons/icon.ico")
        .set_manifest(r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
<dependency><dependentAssembly><assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/></dependentAssembly></dependency>
</assembly>"#)
        .compile()
        .expect("compile application resources");
    println!("cargo:rerun-if-changed=../desktop/src-tauri/icons/icon.ico");
}
