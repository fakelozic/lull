//! Compiles the window's .slint files into Rust, so nothing is interpreted at run time.

fn main() {
    // Fonts are embedded, so the window looks the same without the user's font folder.
    let config = slint_build::CompilerConfiguration::new()
        .embed_resources(slint_build::EmbedResourcesKind::EmbedFiles)
        .with_style("fluent-dark".into());
    if let Err(error) = slint_build::compile_with_config("ui/app.slint", config) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
