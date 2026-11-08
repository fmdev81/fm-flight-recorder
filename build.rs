// Link setup for the FileMaker glue (`--features fmx`).
//
// FMWrapper.framework comes from the Claris plug-in SDK (Libraries/Mac) or,
// because only the exported C symbols are needed to link, from the copy
// inside FileMaker Pro.app. Search order:
//   1. $FMWRAPPER_FRAMEWORK_DIR (folder that contains FMWrapper.framework)
//   2. ./sdk/PlugInSDK/Libraries/Mac  (unpacked Claris SDK, not committed)
//   3. /Applications/FileMaker Pro.app/Contents/Frameworks
// At run time FileMaker has already loaded its own FMWrapper; the rpath
// @executable_path/../Frameworks points there.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=FMWRAPPER_FRAMEWORK_DIR");
    if std::env::var_os("CARGO_FEATURE_FMX").is_none() {
        return;
    }
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        panic!("the fmx feature is only wired for macOS in this prototype");
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("FMWRAPPER_FRAMEWORK_DIR") {
        candidates.push(d.into());
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    candidates.push(manifest.join("sdk/PlugInSDK/Libraries/Mac"));
    candidates.push("/Applications/FileMaker Pro.app/Contents/Frameworks".into());

    let dir = candidates
        .into_iter()
        .find(|d| d.join("FMWrapper.framework").exists())
        .expect("FMWrapper.framework not found; set FMWRAPPER_FRAMEWORK_DIR");
    println!("cargo:warning=linking FMWrapper.framework from {}", dir.display());
    println!("cargo:rustc-link-search=framework={}", dir.display());
    // SDK sample uses @loader_path/../Frameworks; the app's own copy is at
    // @executable_path/../Frameworks. Both are harmless.
    println!("cargo:rustc-cdylib-link-arg=-Wl,-rpath,@executable_path/../Frameworks");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-rpath,@loader_path/../Frameworks");
    println!("cargo:rustc-cdylib-link-arg=-Wl,-install_name,@rpath/FlightRecorder");
}
