fn main() {
    slint_build::compile("ui/app.slint").expect("Failed to compile Slint UI template");

    #[cfg(windows)]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("assets/app_icon.ico");
        res.compile().expect("Failed to compile Windows resource");
    }
}
