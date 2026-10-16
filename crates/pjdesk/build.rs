//! Embeds the app icon and version metadata into the Windows executable.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/pjdesk.ico");
        res.set("ProductName", "PJDesk");
        res.set("FileDescription", "PJDesk remote desktop");
        res.set("CompanyName", "PJDesk");
        // Cross-compiling from Linux: use the MinGW resource compiler.
        if std::env::var_os("WINDRES").is_none() {
            res.set_windres_path("x86_64-w64-mingw32-windres");
        }
        if let Err(e) = res.compile() {
            println!("cargo:warning=app icon embedding skipped: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/pjdesk.ico");
}
