//! Windows resources of `codenotch.exe`: the application icon (resource id 1, which gpui also
//! loads for its windows) and a VERSIONINFO block from the package version.
//!
//! `assets/codenotch.ico` is generated from the tray mark rasteriser by the test
//! `icon::tests::committed_icon_matches_the_tray_mark` (set `CODENOTCH_WRITE_ICON=1` to rewrite
//! it). A missing resource compiler only produces a warning: the exe is then built without
//! icon and version information.

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/codenotch.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let out_dir = match std::env::var("OUT_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => return,
    };
    let version = std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".into());
    let mut nums: Vec<u16> = version
        .split(['.', '-', '+'])
        .take(3)
        .map(|p| p.parse().unwrap_or(0))
        .collect();
    nums.resize(4, 0);
    let commas = nums
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let icon = manifest_dir.join("assets").join("codenotch.ico");
    let icon_line = if icon.is_file() {
        format!(
            "1 ICON \"{}\"\n",
            icon.display().to_string().replace('\\', "\\\\")
        )
    } else {
        println!(
            "cargo:warning=assets/codenotch.ico missing: building without an application icon"
        );
        String::new()
    };
    let rc = format!(
        r#"#pragma code_page(65001)
{icon_line}
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
    BLOCK "StringFileInfo"
    BEGIN
        BLOCK "040904b0"
        BEGIN
            VALUE "CompanyName", "Kev"
            VALUE "FileDescription", "CodeNotch"
            VALUE "FileVersion", "{version}"
            VALUE "InternalName", "codenotch"
            VALUE "LegalCopyright", "MIT licence"
            VALUE "OriginalFilename", "codenotch.exe"
            VALUE "ProductName", "CodeNotch for Windows"
            VALUE "ProductVersion", "{version}"
        END
    END
    BLOCK "VarFileInfo"
    BEGIN
        VALUE "Translation", 0x409, 1200
    END
END
"#
    );
    let rc_path = out_dir.join("codenotch.rc");
    if let Err(err) = std::fs::write(&rc_path, rc) {
        println!("cargo:warning=cannot write the resource script: {err}");
        return;
    }
    match embed_resource::compile(&rc_path, embed_resource::NONE) {
        embed_resource::CompilationResult::Ok | embed_resource::CompilationResult::NotWindows => {}
        other => println!("cargo:warning=no icon/version resource: {other}"),
    }
}
