fn main() {
    println!("cargo::rustc-link-arg-bin=init=--script=user/user.ld");
}
