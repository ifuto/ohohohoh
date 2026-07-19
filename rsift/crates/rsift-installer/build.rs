fn main() {
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let prebuilt = manifest_dir
        .join("../..")
        .join("bootstrap/prebuilt/rsift-bootstrap.jar");
    if prebuilt.exists() {
        println!("cargo:rerun-if-changed={}", prebuilt.display());
    }
}
