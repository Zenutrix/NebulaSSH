fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "NebulaSSH");
        res.set("FileDescription", "NebulaSSH – SSH, SFTP und Serial");
        res.compile().expect("Icon-Ressource");
    }
}
