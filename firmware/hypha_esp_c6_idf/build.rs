fn main() {
    embuild::espidf::sysenv::output();

    println!("cargo:rustc-check-cfg=cfg(esp_idf_mbedtls_certificate_bundle)");
    println!("cargo:rerun-if-env-changed=OTA_PUBKEY_HEX");
    println!("cargo:rerun-if-env-changed=OTA_PUBKEY_PATH");
    if let Ok(path) = std::env::var("OTA_PUBKEY_PATH") {
        println!("cargo:rerun-if-changed={path}");
        let pubkey_hex = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read OTA_PUBKEY_PATH {path}: {e}"));
        println!("cargo:rustc-env=OTA_PUBKEY_HEX={}", pubkey_hex.trim());
    }
}
