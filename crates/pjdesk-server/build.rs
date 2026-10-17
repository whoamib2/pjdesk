//! Embeds the server icon and version metadata into the Windows executable.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/pjdesk-server.ico");
        res.set("ProductName", "PJDesk Server");
        res.set("FileDescription", "PJDesk remote desktop server");
        res.set("CompanyName", "PJDesk");
        if std::env::var_os("WINDRES").is_none() {
            res.set_windres_path("x86_64-w64-mingw32-windres");
        }
        if let Err(e) = res.compile() {
            println!("cargo:warning=server icon embedding skipped: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/pjdesk-server.ico");
}
