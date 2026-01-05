//! Build script for Background Muter
//! Sets up Windows-specific resources like the application icon

use std::env;
use std::path::{Path, PathBuf};

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

    // Prefer a checked-in ICO if the user provides one.
    let repo_ico_path = Path::new("assets/icon.ico");
    let generated_ico_path = out_dir.join("bg-muter.ico");
    let ico_path: PathBuf = if repo_ico_path.exists() {
        repo_ico_path.to_path_buf()
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
        if let Some(p) = ico_path.to_str() {
            res.set_icon(p);
        }
    }

    // Compile the resource
    if let Err(e) = res.compile() {
        eprintln!("Warning: Failed to compile Windows resources: {}", e);
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
