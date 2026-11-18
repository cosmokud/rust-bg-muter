//! Build script for Rust Background Muter
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
  <description>Rust Background Muter</description>
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
    // Tell cargo to rerun if the build script or icon changes. `src` is watched on purpose: it
    // is what lets a build in a checkout other than the one that last linked relink its own
    // binary at the shared target dir (see .githooks/post-checkout and scripts/build-release.ps1).
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=Cargo.toml");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=assets/icon.png");
    println!("cargo:rerun-if-changed=assets/icon.ico");

    // A release build links over `rust-bg-muter.exe`, which Windows will not allow while the
    // app is up: the link fails with "Access is denied" and the build has to be run again.
    // Ending a running copy here, before the crate is linked, is what lets
    // `cargo build --release` replace the binary in one pass.
    #[cfg(target_os = "windows")]
    if env::var("PROFILE").as_deref() == Ok("release") {
        stop_running_app();
    }

    // The commit this build is of, stamped into the executable so a binary can be told from
    // the one another checkout linked over it at the shared target dir.
    let commit = the_commit();
    println!("cargo:rustc-env=BGM_COMMIT={commit}");

    // This build script is only meaningful when the *target* is Windows.
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR not set"));
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    // Prefer a checked-in ICO if the user provides one.
    let repo_ico_path = Path::new("assets/icon.ico");
    let generated_ico_path = out_dir.join("rust-bg-muter.ico");
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
    res.set("ProductName", "Rust Background Muter");
    res.set("FileDescription", "Rust Background Muter");
    res.set("LegalCopyright", "MIT License");
    res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
    res.set("FileVersion", env!("CARGO_PKG_VERSION"));
    // Read back as `(Get-Item <exe>).VersionInfo.Comments`, which is how the build tells a
    // binary of this checkout from one another checkout left at the same path.
    res.set("Comments", commit.as_str());

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
    let manifest_path = out_dir.join("rust-bg-muter.manifest");
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

/// The commit this build is of, or `"unknown"` where git cannot say: a source tarball has no
/// git and still has to build, and a stamp nobody can read has to compare unequal to every
/// HEAD.
fn the_commit() -> String {
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR"))
        .stderr(std::process::Stdio::null())
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        .unwrap_or_default();
    if commit.is_empty() {
        "unknown".to_owned()
    } else {
        commit
    }
}

/// Ends a running copy of the app so a release build can replace its binary.
/// Nothing is reported when the app is not running, which is the usual case.
#[cfg(target_os = "windows")]
fn stop_running_app() {
    use std::os::windows::process::CommandExt;

    /// CREATE_NO_WINDOW, so a build started from an IDE shows no console flash.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let image = concat!(env!("CARGO_PKG_NAME"), ".exe");
    let stopped = std::process::Command::new("taskkill")
        .args(["/F", "/IM", image])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .is_ok_and(|status| status.success());

    if stopped {
        println!("cargo:warning=Stopped the running {image} before rebuilding");
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
