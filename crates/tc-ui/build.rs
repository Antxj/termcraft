//! Windows: the program icon goes into termcraft.exe as a resource, so
//! Explorer, shortcuts and taskbar pins show it.

fn main() {
    println!("cargo:rerun-if-changed=assets/termcraft.rc");
    println!("cargo:rerun-if-changed=assets/termcraft.ico");
    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile("assets/termcraft.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("compile the Windows resources");
    }
}
