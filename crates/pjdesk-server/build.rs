//! Embeds a `requireAdministrator` manifest in pjdesk-server.exe, so the server
//! (its management window, service control and account management) always runs
//! elevated. Only runs for Windows targets; a no-op elsewhere.

use std::process::Command;

fn main() {
    let target = std::env::var("TARGET").unwrap_or_default();
    if !target.contains("windows") {
        return;
    }
    println!("cargo:rerun-if-changed=pjdesk-server.rc");
    println!("cargo:rerun-if-changed=pjdesk-server.manifest");

    let out_dir = std::env::var("OUT_DIR").unwrap();
    let obj = format!("{out_dir}/manifest.o");

    // Pick the resource compiler for the target.
    let windres = if target.contains("gnu") {
        std::env::var("WINDRES").unwrap_or_else(|_| "x86_64-w64-mingw32-windres".to_string())
    } else {
        // MSVC toolchains ship rc.exe; let the `embed-resource`-style fallback
        // be handled by the linker-less path (skip, the GNU build is primary).
        println!("cargo:warning=manifest embedding only wired for the gnu target; skipping");
        return;
    };

    match Command::new(&windres).args(["pjdesk-server.rc", "-O", "coff", "-o", &obj]).status() {
        Ok(status) if status.success() => {
            println!("cargo:rustc-link-arg={obj}");
        }
        other => {
            println!("cargo:warning=windres ({windres}) did not embed the manifest: {other:?}");
        }
    }
}
