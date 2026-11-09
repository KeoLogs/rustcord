//! Build script: on Windows targets, embed the app icon and version info
//! (from Cargo.toml) into rustcord.exe. Other targets need nothing.
//!
//! Uses `CARGO_CFG_TARGET_OS` rather than `#[cfg(windows)]`, because build
//! scripts run on the host (cross-compiling from Linux needs mingw-w64's
//! `windres`, see the winresource docs).

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/icon/rustcord.ico");
    println!("cargo:rerun-if-env-changed=RUSTCORD_REQUIRE_WINRES");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/rustcord.ico");
        if let Err(e) = res.compile() {
            // A missing resource compiler shouldn't stop a dev build, but the
            // release workflow sets RUSTCORD_REQUIRE_WINRES so a release exe
            // never ships without its icon.
            if std::env::var_os("RUSTCORD_REQUIRE_WINRES").is_some() {
                panic!("could not embed the Windows icon / version info: {e}");
            }
            println!("cargo:warning=could not embed the Windows icon: {e}");
        }
    }
}
