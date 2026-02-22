fn main() {
    let target = std::env::var("TARGET").unwrap();
    println!("cargo:rustc-env=ROOTCX_TARGET={target}");
}
