// Embeds the icon and version info into the Windows executables.
fn main() {
    println!("cargo:rerun-if-changed=assets/icons/bticino-easykit.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icons/bticino-easykit.ico");
        res.set("ProductName", "Door entry for BTicino EASYKIT");
        res.set("FileDescription", "Unofficial BTicino EASYKIT client");
        if let Err(e) = res.compile() {
            println!("cargo:warning=icon not embedded: {e}");
        }
    }
}
