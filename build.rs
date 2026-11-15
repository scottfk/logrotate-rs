//! Link the system libraries upstream logrotate links (Gentoo USE="acl"):
//! popt for the command line and for splitting argument strings, and libacl.

fn main() {
    println!("cargo:rustc-link-lib=popt");
    println!("cargo:rustc-link-lib=acl");
    println!("cargo:rerun-if-changed=build.rs");
}
