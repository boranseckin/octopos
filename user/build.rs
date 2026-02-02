fn main() {
    println!("cargo::rustc-link-arg-bins=--script=user/user.ld");
}
