//! Puts the Maarse icon and version information into the Windows exe file.
//! Skipped for other targets; if the resource compiler is missing the build goes on without an icon.

/// The language files in `i18n/` are built into the program; this writes the list.
fn builtin_languages() {
    let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default()).join("../i18n");
    println!("cargo:rerun-if-changed={}", dir.display());
    let mut files: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), p)))
        .collect();
    files.sort();
    let mut code = String::from("pub const BUILTIN: &[(&str, &str)] = &[\n");
    for (name, path) in &files {
        println!("cargo:rerun-if-changed={}", path.display());
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
        let path = path.display().to_string();
        let path = path.strip_prefix(r"\\?\").unwrap_or(&path);
        code.push_str(&format!("    ({name:?}, include_str!({path:?})),\n"));
    }
    code.push_str("];\n");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap_or_default()).join("i18n_builtin.rs");
    let _ = std::fs::write(out, code);
}

fn main() {
    builtin_languages();
    println!("cargo:rerun-if-changed=../packaging/windows/maarse.ico");
    println!("cargo:rerun-if-env-changed=MAARSE_VERSION");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = std::env::var("MAARSE_VERSION").unwrap_or_else(|_| std::env::var("CARGO_PKG_VERSION").unwrap_or_default());
    let mut parts = version.split(['.', '-', '+']).map(|p| p.parse::<u64>().unwrap_or(0));
    let (major, minor, patch) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    let numeric = major << 48 | minor << 32 | patch << 16;
    let mut res = winresource::WindowsResource::new();
    res.set_icon("../packaging/windows/maarse.ico");
    res.set("ProductName", "Må:rse");
    res.set("FileDescription", "Må:rse tray app");
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set_version_info(winresource::VersionInfo::FILEVERSION, numeric);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not add the icon to the exe: {e}");
    }
}
