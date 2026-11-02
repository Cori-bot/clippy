//! Embeds the app logo as the daemon's icon (`clippy-daemon.rc`).
fn main() {
    println!("cargo:rerun-if-changed=clippy-daemon.rc");
    println!("cargo:rerun-if-changed=../clippy-ui/icons/icon.ico");
    embed_resource::compile("clippy-daemon.rc", embed_resource::NONE)
        .manifest_optional()
        .expect("embedding the daemon icon");
}
