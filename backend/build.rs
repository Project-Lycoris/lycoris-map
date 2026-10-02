fn main() {
    // SQLx embeds the migration directory. A newly added migration must rebuild
    // the binary even when Rust source files and older SQL files are unchanged.
    println!("cargo:rerun-if-changed=migrations");
}
