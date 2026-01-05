//! Build script for Background Muter
//! Sets up Windows-specific resources like the application icon

fn main() {
    // Only run on Windows
    #[cfg(target_os = "windows")]
    {
        // Set up Windows resource file for icon and metadata
        if std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default() == "windows" {
            let mut res = winres::WindowsResource::new();
            
            // Application metadata
            res.set("ProductName", "Background Muter");
            res.set("FileDescription", "Automatically mute background applications");
            res.set("LegalCopyright", "MIT License");
            res.set("ProductVersion", env!("CARGO_PKG_VERSION"));
            res.set("FileVersion", env!("CARGO_PKG_VERSION"));
            
            // If we had an icon file, we'd set it here:
            // res.set_icon("assets/icon.ico");
            
            // Compile the resource
            if let Err(e) = res.compile() {
                eprintln!("Warning: Failed to compile Windows resources: {}", e);
            }
        }
    }
    
    // Tell cargo to rerun if the build script changes
    println!("cargo:rerun-if-changed=build.rs");
}
