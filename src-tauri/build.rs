fn main() {
    // Direct Cargo builds must also re-embed changed frontend files.
    println!("cargo:rerun-if-changed=../src/ui");
    println!("cargo:rerun-if-changed=capabilities");
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
