fn main() {
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-arg=/EXPORT:NvOptimusEnablement");
    #[cfg(target_os = "windows")]
    println!("cargo:rustc-link-arg=/EXPORT:AmdPowerXpressRequestHighPerformance");
}
