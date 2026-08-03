fn main() {
    println!("cargo:rerun-if-changed=assets/Info.plist");

    #[cfg(target_os = "macos")]
    {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
        let template = std::path::Path::new(&manifest_dir).join("assets/Info.plist");
        let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION");
        let contents = std::fs::read_to_string(template)
            .expect("read Info.plist")
            .replace("@VERSION@", &version);
        let plist =
            std::path::Path::new(&std::env::var("OUT_DIR").expect("OUT_DIR")).join("Info.plist");
        std::fs::write(&plist, contents).expect("write generated Info.plist");
        println!(
            "cargo:rustc-link-arg=-Wl,-sectcreate,__TEXT,__info_plist,{}",
            plist.display()
        );
    }
}
