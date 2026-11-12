fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent-dark".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI failed to compile");

    // the exe's own icon (Explorer, shortcuts, taskbar) and version info
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "В кадре")
            .set("FileDescription", "В кадре — автоматический оператор для OBS")
            .set("CompanyName", "VKadre");
        if let Err(e) = res.compile() {
            // a missing resource compiler must not break the build; the window icon still works
            println!("cargo:warning=exe icon not embedded: {e}");
        }
    }
}
