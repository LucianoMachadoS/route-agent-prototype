// Embeds the icon and version info into gameprep.exe, so Explorer, the
// taskbar and the MSI's Add/Remove Programs entry all show the real icon.
// The icon itself is generated from src/icon.rs (no binary assets).

#[path = "src/icon.rs"]
#[allow(dead_code)]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let ico = out.join("gameprep.ico");
    std::fs::write(&ico, icon::ico(&[16, 24, 32, 48, 64, 256])).unwrap();

    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap())
        .set("ProductName", "GamePrep")
        .set(
            "FileDescription",
            "GamePrep - otimizador de Windows para jogos",
        )
        .set("CompanyName", "Luciano Machado")
        .set("LegalCopyright", "MIT License");
    if let Err(e) = res.compile() {
        // Missing rc.exe shouldn't break the build - just no icon.
        println!("cargo:warning=no icon/version resource embedded: {e}");
    }
}
