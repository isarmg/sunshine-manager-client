fn main() {
    let target = std::env::var("TARGET").expect("Cargo target");
    assert!(
        matches!(
            target.as_str(),
            "x86_64-unknown-linux-gnu"
                | "x86_64-pc-windows-msvc"
                | "x86_64-pc-windows-gnu"
                | "x86_64-apple-darwin"
                | "aarch64-apple-darwin"
        ),
        "Sunshine Client supports Linux/Windows x86_64 and macOS x86_64/aarch64"
    );
    println!("cargo:rerun-if-env-changed=SUNSHINE_CLIENT_BUILD_SHA");
    let sha = std::env::var("SUNSHINE_CLIENT_BUILD_SHA").unwrap_or_else(|_| "development".into());
    assert!(
        sha == "development"
            || (sha.len() == 40
                && sha
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())),
        "Client build identity must be a full lowercase Git SHA"
    );
    println!("cargo:rustc-env=SUNSHINE_CLIENT_BUILD_SHA={sha}");
}
