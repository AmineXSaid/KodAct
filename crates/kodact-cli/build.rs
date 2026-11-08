//! On Windows, Kod, KodAct's logo (see `design/brand`), becomes kodact.exe's icon (`assets/kodact.rc`).
//! A build without a resource compiler fails here rather than ship no icon.

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=assets/kodact.rc");
        println!("cargo:rerun-if-changed=assets/kodact.ico");
        embed_resource::compile("assets/kodact.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("the kodact icon (assets/kodact.rc) could not be compiled");
    }
}
