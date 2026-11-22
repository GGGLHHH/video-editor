//! 把程序图标（`assets/icon/app.rc`）编进 Windows 的 exe。其他平台不用。

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/app.rc");
    println!("cargo:rerun-if-changed=assets/icon/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && let Err(e) =
            embed_resource::compile("assets/icon/app.rc", embed_resource::NONE).manifest_optional()
    {
        println!("cargo:warning=程序图标没能编进 exe：{e}");
        std::process::exit(1);
    }
}
