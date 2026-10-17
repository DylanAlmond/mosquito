fn main() {
    // The tauri-pilot plugin is debug tooling (issue #7; see
    // harness/desktop/README.md): its dependency is optional behind the
    // `pilot` feature, which stays out of `default` so release builds
    // (tauri-action CI, plain `cargo build --release`) never link any
    // pilot code. Cargo has no profile-gated dependencies
    // (`debug_assertions` is not a supported cfg in target tables), so
    // this feature pairing is the documented equivalent.
    //
    // The same gate applies to the plugin's capability: without the
    // feature the plugin is not in the dependency graph, so resolving
    // its `pilot:default` permission would fail the build. The
    // feature-gated glob keeps `capabilities/pilot.toml` (whose TOML
    // comment documents why the grant is inert in release) out of
    // non-pilot builds and out of release entirely.
    //
    // Custom patterns must emit rerun-if-changed themselves
    // (tauri_build::Attributes::capabilities_path_pattern).
    println!("cargo:rerun-if-changed=capabilities");

    let pattern = if std::env::var_os("CARGO_FEATURE_PILOT").is_some() {
        "./capabilities/**/*"
    } else {
        // Dev-tooling manifests stay in TOML; packaged capabilities are
        // JSON. If a packaged capability ever needs TOML, move it under
        // the pilot branch or reconsider the split first.
        "./capabilities/**/*.json"
    };

    tauri_build::try_build(tauri_build::Attributes::new().capabilities_path_pattern(pattern))
        .expect("failed to run tauri-build");
}
