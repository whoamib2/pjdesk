//! Embeds the server icon and version metadata into the Windows executable.

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/pjdesk-server.ico");
        res.set("ProductName", "PJDesk Server");
        res.set("FileDescription", "PJDesk remote desktop server");
        res.set("CompanyName", "PJDesk");
        res.set("LegalCopyright", "PJDesk");
        res.set("InternalName", "pjdesk-server");
        res.set("OriginalFilename", "pjdesk-server.exe");
        res.set_manifest(&manifest());
        if std::env::var_os("WINDRES").is_none() {
            res.set_windres_path("x86_64-w64-mingw32-windres");
        }
        if let Err(e) = res.compile() {
            println!("cargo:warning=server icon embedding skipped: {e}");
        }
    }
    println!("cargo:rerun-if-changed=assets/pjdesk-server.ico");
}

fn version_quad() -> String {
    let v = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut parts: Vec<String> = v
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .next()
        .unwrap_or("0.0.0")
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();
    while parts.len() < 4 {
        parts.push("0".into());
    }
    parts.truncate(4);
    parts.join(".")
}

fn manifest() -> String {
    let version = version_quad();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="PJDesk.Server" version="{version}" processorArchitecture="amd64"/>
  <description>PJDesk remote desktop server</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
</assembly>
"#
    )
}
