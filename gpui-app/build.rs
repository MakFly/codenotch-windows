fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=codenotch.rc");
        println!("cargo:rerun-if-changed=assets/app/icon.ico");
        embed_resource::compile("codenotch.rc", embed_resource::NONE)
            .manifest_optional()
            .expect("app icon resource");
    }
}
