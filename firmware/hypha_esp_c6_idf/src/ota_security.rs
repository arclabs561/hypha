//! Pure helpers for signed HTTP OTA.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedManifest {
    pub version: String,
    pub n_chunks: u32,
    pub hash_hex: String,
}

pub const OTA_IDLE: u8 = 0;
pub const OTA_DISABLED: u8 = 1;
pub const OTA_NO_MANIFEST: u8 = 2;
pub const OTA_BAD_KEY: u8 = 3;
pub const OTA_BAD_MANIFEST: u8 = 4;
pub const OTA_NOT_NEWER: u8 = 5;
pub const OTA_DOWNLOADING: u8 = 6;
pub const OTA_FETCH_ERROR: u8 = 7;
pub const OTA_HASH_MISMATCH: u8 = 8;
pub const OTA_CHUNK_MISMATCH: u8 = 9;
pub const OTA_APPLY_ERROR: u8 = 10;
pub const OTA_REBOOTING: u8 = 11;

pub fn ota_state_name(state: u8) -> &'static str {
    match state {
        OTA_IDLE => "idle",
        OTA_DISABLED => "disabled",
        OTA_NO_MANIFEST => "no_manifest",
        OTA_BAD_KEY => "bad_key",
        OTA_BAD_MANIFEST => "bad_manifest",
        OTA_NOT_NEWER => "not_newer",
        OTA_DOWNLOADING => "downloading",
        OTA_FETCH_ERROR => "fetch_error",
        OTA_HASH_MISMATCH => "hash_mismatch",
        OTA_CHUNK_MISMATCH => "chunk_mismatch",
        OTA_APPLY_ERROR => "apply_error",
        OTA_REBOOTING => "rebooting",
        _ => "unknown",
    }
}

/// Parse a plain `X.Y.Z` version into a comparable tuple.
pub fn parse_semver(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split('.');
    let major = it.next()?.parse().ok()?;
    let minor = it.next()?.parse().ok()?;
    let patch = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

/// True only when both versions are plain semver and `staged` is newer.
pub fn is_strictly_newer(staged: &str, running: &str) -> bool {
    matches!(
        (parse_semver(staged), parse_semver(running)),
        (Some(staged), Some(running)) if staged > running
    )
}

/// The signed manifest that authenticates an HTTP OTA image.
pub fn manifest_url_for(ota_url: &str) -> String {
    format!("{}.manifest.json", ota_url)
}

/// Decode a 32-byte Ed25519 public key from hex.
pub fn decode_pubkey_hex(hex: &str) -> Option<[u8; 32]> {
    let bytes = hex::decode(hex.trim()).ok()?;
    bytes.try_into().ok()
}

/// Verify a manifest and return the signed fields needed by the HTTP OTA path.
pub fn verify_signed_manifest(bytes: &[u8], pubkey: &[u8; 32]) -> Option<VerifiedManifest> {
    let (version, n_chunks, hash_hex, _sig_b64) =
        hypha_ota::protocol::verify_manifest_json_full(bytes, pubkey)?;
    Some(VerifiedManifest {
        version,
        n_chunks,
        hash_hex,
    })
}
