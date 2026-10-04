//! Windows 下给 exe 加上图标和版本信息（任务管理器、文件属性里显示“AI 小说工坊”）。

use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let manifest = PathBuf::from(env("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env("OUT_DIR"));
    // rc.exe 在 .rc 所在目录运行，图标复制过去用相对路径，避免中文路径的编码问题
    std::fs::copy(manifest.join("assets").join("icon.ico"), out.join("icon.ico")).expect("复制图标");
    let (major, minor, patch, version) = (env("CARGO_PKG_VERSION_MAJOR"), env("CARGO_PKG_VERSION_MINOR"), env("CARGO_PKG_VERSION_PATCH"), env("CARGO_PKG_VERSION"));
    let rc = format!(
        r#"#pragma code_page(65001)
1 ICON "icon.ico"
1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "080404B0"
    BEGIN
      VALUE "FileDescription", "AI 小说工坊"
      VALUE "ProductName", "AI 小说工坊"
      VALUE "FileVersion", "{version}"
      VALUE "ProductVersion", "{version}"
      VALUE "LegalCopyright", "AGPL-3.0-or-later"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x804, 1200
  END
END
"#
    );
    let rc_path = out.join("app.rc");
    std::fs::write(&rc_path, rc).expect("写入资源文件");
    embed_resource::compile(&rc_path, embed_resource::NONE).manifest_optional().expect("编译 Windows 资源（图标）失败");
}
