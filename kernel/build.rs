fn main() {
    println!("cargo::rustc-link-arg-bin=octopos=--script=kernel/kernel.ld");
}
