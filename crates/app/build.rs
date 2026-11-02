fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app-icon/gitwarp.ico");

    // The build-dependency is only compiled when the host is Windows, so the
    // call has to be gated on the host as well as on the target.
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_icon();
    }
}

#[cfg(windows)]
fn embed_icon() {
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR is set for build scripts");
    let icon = std::path::Path::new(&manifest_dir)
        .join("assets")
        .join("app-icon")
        .join("gitwarp.ico");
    winresource::WindowsResource::new()
        .set_icon(icon)
        .compile()
        .expect("failed to embed the Windows icon");
}
