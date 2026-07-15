use std::env;
use std::path::PathBuf;
use std::process::Command;

fn pkg_config_cflags(pkg: &str) -> Vec<String> {
    Command::new("pkg-config")
        .args(["--cflags", pkg])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .split_whitespace()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn pkg_config_var(pkg: &str, var: &str) -> Option<String> {
    Command::new("pkg-config")
        .args(["--variable", var, pkg])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Trouve le dossier contenant `obs-module.h`. `libobs.pc` existe sur la
/// plupart des distributions (Arch/Debian/Fedora), mais son `Cflags:` ne
/// pointe pas forcément vers le bon dossier (sur Arch, il donne
/// `-I/usr/include` alors que le header est dans `/usr/include/obs/`) — donc
/// on essaie plusieurs emplacements candidats plutôt que de se fier
/// aveuglément à pkg-config. `OBS_INCLUDE_DIR` permet de forcer l'emplacement
/// si l'auto-détection échoue (installation non standard).
fn find_obs_include_dir() -> PathBuf {
    if let Ok(dir) = env::var("OBS_INCLUDE_DIR") {
        return PathBuf::from(dir);
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(includedir) = pkg_config_var("libobs", "includedir") {
        candidates.push(PathBuf::from(&includedir).join("obs"));
        candidates.push(PathBuf::from(includedir));
    }
    candidates.push(PathBuf::from("/usr/include/obs"));
    candidates.push(PathBuf::from("/usr/include"));
    candidates.push(PathBuf::from("/usr/local/include/obs"));

    candidates
        .into_iter()
        .find(|dir| dir.join("obs-module.h").is_file())
        .unwrap_or_else(|| {
            panic!(
                "could not find obs-module.h anywhere. Install the OBS Studio development \
                 headers first (package `obs-studio` on Arch, `libobs-dev` on Debian/Ubuntu, \
                 `obs-studio-devel` on Fedora — see README.md). If they're installed \
                 in a non-standard location, set OBS_INCLUDE_DIR to that directory and re-run."
            )
        })
}

fn main() {
    println!("cargo:rustc-link-lib=dylib=obs");
    println!("cargo:rustc-link-lib=dylib=obs-frontend-api");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=src/dock.cpp");
    println!("cargo:rerun-if-env-changed=OBS_INCLUDE_DIR");

    let obs_include_dir = find_obs_include_dir();

    let qt_cflags = pkg_config_cflags("Qt6Widgets");
    if qt_cflags.is_empty() {
        panic!(
            "`pkg-config --cflags Qt6Widgets` returned nothing. Install the Qt6 development \
             headers first (package `qt6-base` on Arch, `qt6-base-dev` on Debian/Ubuntu, \
             `qt6-qtbase-devel` on Fedora — see README.md), and make sure pkg-config \
             is installed."
        );
    }

    // Dock Qt natif (src/dock.cpp) — widgets Qt6 seulement, connexions par
    // lambda, donc aucune étape moc nécessaire : une compilation C++ classique
    // suffit. Les flags Qt6 viennent de `pkg-config --cflags Qt6Widgets` (pas
    // de chemins figés), pour compiler tel quel sur n'importe quelle
    // distribution qui fournit un `Qt6Widgets.pc` correct.
    let mut cpp_build = cc::Build::new();
    cpp_build.cpp(true).file("src/dock.cpp").flag_if_supported("-std=c++17");
    for flag in &qt_cflags {
        cpp_build.flag(flag);
    }
    cpp_build.compile("loopwire_dock");

    println!("cargo:rustc-link-lib=dylib=Qt6Widgets");
    println!("cargo:rustc-link-lib=dylib=Qt6Gui");
    println!("cargo:rustc-link-lib=dylib=Qt6Core");

    let bindings = bindgen::Builder::default()
        .header("wrapper.h")
        .clang_arg(format!("-I{}", obs_include_dir.display()))
        .allowlist_function("obs_frontend_add_tools_menu_item")
        .allowlist_function("obs_frontend_add_dock_by_id")
        .allowlist_function("obs_get_proc_handler")
        .allowlist_function("proc_handler_call")
        .allowlist_function("calldata_get_data")
        .allowlist_function("calldata_set_data")
        .allowlist_function("obs_data_get_string")
        .allowlist_function("obs_data_get_int")
        .allowlist_function("obs_data_get_bool")
        .allowlist_function("obs_data_set_string")
        .allowlist_function("obs_data_set_int")
        .allowlist_function("obs_data_set_bool")
        .allowlist_function("blog")
        .allowlist_type("calldata_t")
        .allowlist_type("calldata")
        .allowlist_type("obs_data_t")
        .allowlist_type("obs_module_t")
        .allowlist_type("proc_handler_t")
        .allowlist_var("LIBOBS_API_MAJOR_VER")
        .allowlist_var("LIBOBS_API_MINOR_VER")
        .allowlist_var("LIBOBS_API_PATCH_VER")
        .generate()
        .expect("échec de génération des bindings libobs");

    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("échec d'écriture des bindings");
}
