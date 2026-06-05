use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

const TEXTDOMAIN: &str = "rill";

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

    compile_translations(out_path);
}

/// Compiles every `po/<lang>.po` into `<OUT_DIR>/locale/<lang>/LC_MESSAGES/rill.mo`
/// so a development build (`cargo run`) picks up translations without a system
/// install. The directory is exported as `RILL_LOCALEDIR_BUILD` for `main` to use
/// as a fallback. Packaged builds install the .mo files to the system locale dir
/// and ignore this. `msgfmt` (gettext tools) is required; if it is missing the
/// build still succeeds with translations simply unavailable in dev.
fn compile_translations(out_path: &Path) {
    println!("cargo:rerun-if-changed=po/");

    let locale_dir = out_path.join("locale");
    let Ok(entries) = fs::read_dir("po") else {
        return;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("po") {
            continue;
        }
        let Some(lang) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };

        let dest_dir = locale_dir.join(lang).join("LC_MESSAGES");
        if let Err(e) = fs::create_dir_all(&dest_dir) {
            println!("cargo:warning=failed to create {}: {e}", dest_dir.display());
            continue;
        }
        let mo = dest_dir.join(format!("{TEXTDOMAIN}.mo"));

        match Command::new("msgfmt")
            .arg(&path)
            .arg("-o")
            .arg(&mo)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                println!(
                    "cargo:warning=msgfmt failed for {} ({status})",
                    path.display()
                );
            }
            Err(e) => {
                println!("cargo:warning=msgfmt not found, translations disabled in dev: {e}");
                return;
            }
        }
    }

    println!(
        "cargo:rustc-env=RILL_LOCALEDIR_BUILD={}",
        locale_dir.display()
    );
}
