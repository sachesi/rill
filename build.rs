use std::env;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out_path = Path::new(&out_dir);

    // Compile GResource
    glib_build_tools::compile_resources(
        &["resources"],
        "resources/rill.gresource.xml",
        "rill.gresource",
    );

    // Re-run if resource files change
    println!("cargo:rerun-if-changed=resources/");
    
    // Set resource path for runtime
    let resource_file = out_path.join("rill.gresource");
    println!("cargo:rustc-env=GRESOURCE_FILE={}", resource_file.display());
}