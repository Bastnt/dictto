fn main() {
    windows_reactor_setup::as_self_contained();
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    let profile = output.ancestors().nth(3).unwrap();
    std::fs::copy("../desktop/src-tauri/icons/icon.ico", profile.join("icon.ico"))
        .expect("copy native application icon");
    println!("cargo:rerun-if-changed=../desktop/src-tauri/icons/icon.ico");
}
