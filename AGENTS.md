# AGENTS.md

## Build & Check Commands

- Build: `cargo build`
- Check: `cargo check`
- Lint: `cargo clippy -- -D warnings`
- Format: `cargo fmt`
- Test all: `cargo test`
- Test single: `cargo test <test_name>` (e.g., `cargo test arch_updates`)

## Architecture

Single-binary Rust CLI that outputs status lines for Polybar. Each module in `src/polybar_module/` implements the `RenderablePolybarModule` trait with `wait_update`, `update`, and `render` methods. Modules are registered in `mod.rs` and selected via CLI (clap). Uses Rust Edition 2024 with nightly toolchain.

## Code Style

- Imports: group std, external, crate (`group_imports = "StdExternalCrate"`, `imports_granularity = "Crate"`)
- Visibility: use `pub(crate)` for internal APIs
- Errors: use `anyhow::Result` for fallible functions; avoid `expect`/`panic` outside tests
- Clippy: pedantic enabled + many restriction lints; tests allow `unwrap`/`expect`/`panic`/indexing
- No comments unless complex; docs required for public items (`missing_docs = "warn"`)
- Conventional commits enforced via pre-commit hook
