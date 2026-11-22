//! Puts the Maarse icon and version information into the Windows exe file.
//! Skipped for other targets; if the resource compiler is missing the build goes on without an icon.

fn main() {
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
    res.set("ProductName", "Maarse");
    res.set("FileDescription", "Maarse service");
    res.set("FileVersion", &version);
    res.set("ProductVersion", &version);
    res.set_version_info(winresource::VersionInfo::FILEVERSION, numeric);
    res.set_version_info(winresource::VersionInfo::PRODUCTVERSION, numeric);
    if let Err(e) = res.compile() {
        println!("cargo:warning=could not add the icon to the exe: {e}");
    }
}
