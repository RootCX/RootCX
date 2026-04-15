fn main() {
    println!("cargo:rerun-if-changed=skills/rootcx");
    tauri_build::build();
}
