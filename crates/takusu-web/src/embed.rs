use rust_embed::Embed;

/// Static frontend assets produced by `vite build` into `web/dist`. Embedded
/// into the binary at compile time so `takusu-web` ships as a single file.
///
/// A placeholder `web/dist/index.html` is committed so the crate compiles
/// before the frontend has been built; `vite build` overwrites it.
#[derive(Embed)]
#[folder = "../../web/dist"]
pub struct Assets;
