fn main() {
    println!("cargo:rerun-if-env-changed=SF_BOOTSTRAP_IMPORT_DIR");
    if std::env::var_os("CARGO_FEATURE_SF_STATIC_STARTUP").is_none() { return; }
    assert_eq!(std::env::var("TARGET").unwrap(), "x86_64-pc-windows-msvc",
        "SF static startup requires the matching x64 MSVC bootstrap import library");
    let directory = std::path::PathBuf::from(std::env::var_os("SF_BOOTSTRAP_IMPORT_DIR")
        .expect("build SF winmm first, then set SF_BOOTSTRAP_IMPORT_DIR"));
    assert!(directory.is_absolute(), "SF_BOOTSTRAP_IMPORT_DIR must be absolute");
    let library = directory.join("ShroudForgeWinmm.lib");
    assert!(library.is_file(), "missing SF-owned ShroudForgeWinmm.lib");
    println!("cargo:rerun-if-changed={}", library.display());
    println!("cargo:rustc-link-search=native={}", directory.display());
}
