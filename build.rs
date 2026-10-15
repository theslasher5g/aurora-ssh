//! Embeds the application icon and version info into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        #[cfg(windows)]
        {
            let mut res = winresource::WindowsResource::new();
            res.set_icon("assets/icon.ico");
            res.set("ProductName", "Aurora");
            res.set("FileDescription", "Aurora – SSH, SFTP, RDP & Serial");
            if let Err(e) = res.compile() {
                println!("cargo:warning=Icon konnte nicht eingebettet werden: {e}");
            }
        }
    }
}
