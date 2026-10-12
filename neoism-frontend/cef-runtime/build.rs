fn main() {
    // Relocatable package: place libcef.so and resource files beside this worker.
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
}
