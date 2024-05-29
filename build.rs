fn main() {
    println!("cargo:rustc-link-arg-bin=octopos=--script=src/kernel.ld");
}
