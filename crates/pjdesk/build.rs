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
    // openh264 is C++. rust-lld does not search GCC's directory for libstdc++.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("linux") {
        link_libstdcpp();
    }
}

fn link_libstdcpp() {
    let Ok(out) = std::process::Command::new("g++").arg("-print-file-name=libstdc++.so").output() else {
        return;
    };
    let path = String::from_utf8_lossy(&out.stdout);
    let path = path.trim();
    if let Some(dir) = std::path::Path::new(path).parent() {
        if dir != std::path::Path::new("") {
            println!("cargo:rustc-link-search=native={}", dir.display());
        }
    }
    println!("cargo:rustc-link-lib=stdc++");
}
