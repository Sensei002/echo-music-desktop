//! Compiles the Slint UI at build time.

fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=ui/theme.slint");
    println!("cargo:rerun-if-changed=ui/components.slint");

    slint_build::compile("ui/app.slint").expect("failed to compile the Slint UI");
}
