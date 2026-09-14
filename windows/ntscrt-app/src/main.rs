fn main() {
    // Dependency-resolution probe; replaced by the real app shell.
    let _ = ntscrt_core::NtscStage::new();
    println!("ntscrt probe");
}
