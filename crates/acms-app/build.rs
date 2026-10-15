//! Gives the Windows executable its icon and its name in Explorer's properties. Elsewhere
//! there is nothing to do: Linux takes the icon from the desktop file.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon/acms.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("../../assets/icon/acms.ico")
        .set("ProductName", "ACMS mk3")
        .set("FileDescription", "ACMS mk3 pixel art editor")
        .set("LegalCopyright", "MIT OR Apache-2.0");
    // A build without the Windows SDK's resource compiler still gives a working program,
    // only without an icon: say so instead of failing.
    if let Err(err) = resource.compile() {
        println!("cargo:warning=the executable gets no icon: {err}");
    }
}
