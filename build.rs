//! Build script for Background Muter
//! Sets up Windows-specific resources like the application icon
//! and embeds the visual styles manifest for modern Windows UI

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Windows visual styles manifest for modern common controls (Windows 11 look)
const VISUAL_STYLES_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity
    version="1.0.0.0"
    processorArchitecture="*"
    name="BgMuter"
    type="win32"
  />
  <description>Background Muter</description>
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
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
      <supportedOS Id="{1f676c76-80e1-4239-95bb-83d0f6d0da78}"/>
      <supportedOS Id="{4a2f28e3-53b9-4441-ba9c-d69d4a4a6e38}"/>
      <supportedOS Id="{35138b9a-5d96-4fbd-8e2d-a2440225f93a}"/>
    </application>
  </compatibility>
  <asmv3:application xmlns:asmv3="urn:schemas-microsoft-com:asm.v3">
    <asmv3:windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    </asmv3:windowsSettings>
  </asmv3:application>
</assembly>
"#;

fn main() {
    // Tell cargo to rerun if the build script or icon changes
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // This build script is only meaningful when the *target* is Windows.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // Prefer a checked-in ICO if the user provides one.
    let repo_ico_path = Path::new("assets/icon.ico");
    let generated_ico_path = out_dir.join("bg-muter.ico");
    let ico_path: PathBuf = if repo_ico_path.exists() {
        // Use absolute path to ensure winres can find it
        std::fs::canonicalize(repo_ico_path).unwrap_or_else(|_| repo_ico_path.to_path_buf())
    } else {
        if let Err(e) = generate_ico(Path::new("assets/icon.png"), &generated_ico_path) {
            eprintln!(
                "Warning: Failed to generate ICO from assets/icon.png: {}",
                e
            );
        }
        generated_ico_path
    };

    // Set up Windows resource file for icon and metadata.
    let mut res = winres::WindowsResource::new();

    // Application metadata
    res.set("ProductName", "Background Muter");
    res.set("FileDescription", "Automatically mute background applications");
    res.set("LegalCopyright", "MIT License");
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));

    if ico_path.exists() {
        // Convert to string, handling the \\?\ prefix from canonicalize on Windows
        let ico_str = ico_path.to_string_lossy();
        let ico_str = ico_str.strip_prefix(r"\\?\").unwrap_or(&ico_str);
        res.set_icon(ico_str);
        println!("cargo:warning=Using icon: {}", ico_str);
    } else {
        println!("cargo:warning=Icon file not found: {}", ico_path.display());
    }

    // Write the visual styles manifest and embed it
    let manifest_path = out_dir.join("bg-muter.manifest");
    if let Ok(mut f) = std::fs::File::create(&manifest_path) {
        let _ = f.write_all(VISUAL_STYLES_MANIFEST.as_bytes());
        let manifest_str = manifest_path.to_string_lossy();
        let manifest_str = manifest_str.strip_prefix(r"\\?\").unwrap_or(&manifest_str);
        res.set_manifest_file(manifest_str);
        println!("cargo:warning=Embedded visual styles manifest");
    }

    // Compile the resource
    // Note: winres outputs cargo:rustc-link-lib=dylib=resource which doesn't work properly
    // We need to link the resource.lib statically instead
    match res.compile() {
        Ok(_) => {
            println!("cargo:warning=Windows resources compiled successfully");
            
            // For MSVC, explicitly link the resource library as a static library
            // This overrides winres's default dylib linking
            if target_env == "msvc" {
                let resource_lib = out_dir.join("resource.lib");
                if resource_lib.exists() {
                    // Link the resource library directly using link.exe arguments
                    println!("cargo:rustc-link-arg={}", resource_lib.display());
                }
            }
        }
        Err(e) => {
            eprintln!("Warning: Failed to compile Windows resources: {}", e);
        }
    }
}

fn generate_ico(png_path: &Path, ico_path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    use ico::{IconDir, IconDirEntry, IconImage, ResourceType};
    use image::imageops::FilterType;

    let img = image::open(png_path)?;

    // Common Windows icon sizes. Including multiple sizes prevents blurry scaling.
    let sizes: &[u32] = &[256, 64, 48, 32, 16];
    let mut icon_dir = IconDir::new(ResourceType::Icon);

    for &size in sizes {
        let resized = img
            .resize_exact(size, size, FilterType::Lanczos3)
            .to_rgba8();
        let icon_image = IconImage::from_rgba_data(size, size, resized.into_raw());
        let entry = IconDirEntry::encode(&icon_image)?;
        icon_dir.add_entry(entry);
    }

    let file = std::fs::File::create(ico_path)?;
    icon_dir.write(file)?;
    Ok(())
}
