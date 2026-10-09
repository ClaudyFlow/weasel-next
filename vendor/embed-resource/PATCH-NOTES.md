# Vendored `embed-resource` (3.0.11)

Upstream: <https://github.com/nabijaczleweli/rust-embed-resource>, crate
`embed-resource` 3.0.11 (MIT), wired in through `[patch.crates-io]` in the
workspace root `Cargo.toml`. Only `src/lib.rs` and `src/windows_not_msvc.rs`
differ from the published crate; every changed hunk is marked with `PATCH:`.

## Why

The `*-pc-windows-gnu` builds on this project run against **llvm-mingw**, whose
`windres`/`llvm-windres` is a GNU-syntax front-end over `llvm-rc`:

1. Its own preprocessing mangles quoted macro values. Passing
   `-DWEASEL_FILE_DESCRIPTION="Weasel-RS Broker"` yields
   `VALUE "FileDescription", Weasel-RS Broker`, and resource compilation fails
   with `expected BLOCK or VALUE, got Weasel-RS`. `build_support/version.rs`
   (shared VERSIONINFO) and `tip/build.rs` (language-bar icons) both rely on
   quoted defines, so nothing in the workspace could build.
2. Upstream routes non-`aarch64` GNU targets through `CompilerType::WindRes`,
   which hands the raw `.rc` plus `-D` defines straight to `windres`.
3. The LLVM-RC path *does* preprocess with the C compiler, but then invokes the
   compiler with llvm-rc flags (`-fo`, `-C`, `-no-preprocess`). `llvm-windres`
   forwards those to `llvm-rc`, which re-runs preprocessing and dies with
   `clang: no such file or directory: ...\out\lib<prefix>.a`.

## What changed

* `Compiler::windres()` now returns `CompilerType::LlvmRc { has_no_preprocess:
  true }`, so `.rc` files for `windres` front-ends are expanded with the C
  compiler (defines and includes keep their quotes) exactly like the LLVM-RC
  path.
* `Compiler::compile()` detects `windres` front-ends (`is_gnu_windres()`) and
  compiles the already expanded file with GNU syntax
  (`--input … --output … --output-format=coff`) instead of llvm-rc's
  `-fo`/`-C`/`-no-preprocess`, which would make `llvm-windres` preprocess a
  second time and fail.
* `ResourceCompiler::new()` supplies the windres target (`--target
  pe-x86-64`/`pe-i386`/`pe-aarch64-little`) for those front-ends too; upstream
  only set it for `CompilerType::WindRes`.
* `CompilerType::WindRes` is now unreachable and carries `#[allow(dead_code)]`;
  it stays because the rest of the upstream code still matches on it.

MSVC targets are untouched: they keep using `RC.EXE` from the Windows SDK.

## Verified with

```powershell
cargo build --workspace --release   # x86_64-pc-windows-gnu, llvm-mingw 22
```
