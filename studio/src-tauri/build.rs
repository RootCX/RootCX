fn main() {
    println!("cargo:rerun-if-changed=../../.agents/instructions/rootcx-sdk.md");
    tauri_build::build();
}
