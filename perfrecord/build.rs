fn main() {
    println!(
        "cargo:rustc-link-search=framework={}",
        "/System/Library/PrivateFrameworks/"
    );
}
