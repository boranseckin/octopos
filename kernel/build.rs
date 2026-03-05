fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is always set by cargo");
    println!("cargo::rustc-link-arg-bin=octopos=--script={manifest_dir}/kernel.ld");
}
