//! Toolchain facts shared by the build scripts.

/// Whether the crate is being built for the MSVC ABI (`link.exe`, `RC.EXE`)
/// rather than a GNU one (`ld`/`windres`, as llvm-mingw provides).
pub fn is_msvc_target() -> bool {
    std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc")
}
