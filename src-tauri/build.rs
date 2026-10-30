fn main() {
    // tauri_build::build();

    let manifest_content = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <dependency>
    <dependentAssembly>
      <assemblyIdentity
        type="win32"
        name="Microsoft.Windows.Common-Controls"
        version="6.0.0.0"
        processorArchitecture="*"
        publicKeyToken="6595b64144ccf1df"
        language="*"
      />
    </dependentAssembly>
  </dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </windowsSettings>
  </application>
</assembly>"#;

    if let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") {
        let manifest_path_buf = std::path::PathBuf::from(&manifest_dir);
        let target_dir = manifest_path_buf.join("target");
        let dist_dir = manifest_path_buf.parent().unwrap_or(&manifest_path_buf).join("dist");

        let dirs_to_sync = [
            target_dir.join("debug"),
            target_dir.join("debug").join("deps"),
            target_dir.join("release"),
            target_dir.join("release").join("deps"),
            dist_dir,
        ];

        let dll_candidates = [
            target_dir.join("debug").join("WebView2Loader.dll"),
            target_dir.join("release").join("WebView2Loader.dll"),
        ];

        let found_dll = dll_candidates.iter().find(|p| p.exists()).cloned();

        for d in &dirs_to_sync {
            if d.exists() {
                // Copy WebView2Loader.dll if available
                if let Some(ref src) = found_dll {
                    let dst = d.join("WebView2Loader.dll");
                    if !dst.exists() {
                        let _ = std::fs::copy(src, &dst);
                    }
                }

                // Write manifest for any exe in the directory
                if let Ok(entries) = std::fs::read_dir(d) {
                    for entry in entries.flatten() {
                        let p = entry.path();
                        if p.extension().map_or(false, |ext| ext == "exe") {
                            let mf = p.with_extension("exe.manifest");
                            let _ = std::fs::write(&mf, manifest_content);
                        }
                    }
                }

                // Also always write agent-aura.exe.manifest in target dirs
                let default_mf = d.join("agent-aura.exe.manifest");
                let _ = std::fs::write(&default_mf, manifest_content);
            }
        }
    }
}
