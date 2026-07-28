fn main() {
    // Same pattern as neoism-window/build.rs: only on a real Windows
    // host (winres needs rc.exe); cross-checks from Linux skip it.
    #[cfg(target_os = "windows")]
    {
        let mut res = winres::WindowsResource::new();
        res.set_icon("../../../misc/windows/neoism.ico");
        res.set("ProductName", "Neoism");
        res.set("FileDescription", "Neoism agent CLI");
        res.compile()
            .expect("Failed to compile Windows resources");
    }
}
