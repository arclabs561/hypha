//! Sign a firmware binary for mesh OTA. Outputs manifest (version, hash, n_chunks, sig) and raw .sig.
//!
//! Usage:
//!   mesh_ota_sign --bin firmware.bin --version 1.0.0 --key privkey.pem
//!   mesh_ota_sign --bin firmware.bin --version 1.0.0 --key privkey.pem --out-dir ./build
//!   mesh_ota_sign --verify --manifest manifest.json --pubkey pubkey.hex
//!
//! Reads privkey.pem (PKCS#8 or raw 32-byte seed hex). Writes:
//!   - manifest.json: { "v", "h", "n", "sig" } for wire format
//!   - firmware.sig: raw 64-byte Ed25519 signature (for embedding)
//!   - pubkey.hex: public key hex (embed in firmware as MESH_OTA_PUBKEY)

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use ed25519_dalek::pkcs8::DecodePrivateKey;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

const CHUNK_SIZE: usize = 128; // match hypha_ota::protocol::CHUNK_SIZE; base64(128)+JSON < 250 ESP-NOW limit

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let mut opts = getopts::Options::new();
    opts.optopt("b", "bin", "firmware binary path", "PATH");
    opts.optopt("v", "version", "version string", "VER");
    opts.optopt("k", "key", "private key path (PKCS#8 PEM or raw 32-byte seed hex)", "PATH");
    opts.optopt("o", "out-dir", "output directory (default: same as bin)", "DIR");
    opts.optopt("m", "manifest", "manifest.json path (for --verify)", "PATH");
    opts.optopt("p", "pubkey", "pubkey.hex path (for --verify)", "PATH");
    opts.optflag("", "verify", "verify manifest with pubkey");
    opts.optflag("h", "help", "print help");

    let m = opts.parse(&args[1..])?;
    if m.opt_present("h") {
        let brief = "Sign firmware for mesh OTA. Use --verify to verify a manifest.";
        print!("{}", opts.usage(&brief));
        return Ok(());
    }

    if m.opt_present("verify") {
        let manifest_path: PathBuf = m.opt_str("manifest").ok_or("--verify requires --manifest")?.into();
        let pubkey_path: PathBuf = m.opt_str("pubkey").ok_or("--verify requires --pubkey")?.into();
        return verify_manifest(&manifest_path, &pubkey_path);
    }

    let bin_path: PathBuf = m.opt_str("bin").ok_or("missing --bin")?.into();
    let version = m.opt_str("version").unwrap_or_else(|| "0.0.0".into());
    let key_path: PathBuf = m.opt_str("key").ok_or("missing --key")?.into();
    let out_dir: PathBuf = m
        .opt_str("out-dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| bin_path.parent().unwrap_or(std::path::Path::new(".")).to_path_buf());

    let bin_data = read_file(&bin_path)?;
    let hash = Sha256::digest(&bin_data);
    let n_chunks = (bin_data.len() + CHUNK_SIZE - 1) / CHUNK_SIZE;
    let payload = build_payload(&version, &hash, n_chunks);

    let signing_key = load_signing_key(&key_path)?;
    let signature = signing_key.sign(&payload);
    let pubkey = signing_key.verifying_key();

    fs::create_dir_all(&out_dir)?;

    // manifest.json for wire
    let manifest = serde_json::json!({
        "ota": "manifest",
        "v": version,
        "h": hex::encode(hash),
        "n": n_chunks,
        "sig": B64.encode(signature.to_bytes())
    });
    let manifest_path = out_dir.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    eprintln!("wrote {}", manifest_path.display());

    // raw .sig for embedding
    let sig_path = out_dir.join("firmware.sig");
    fs::write(&sig_path, signature.to_bytes())?;
    eprintln!("wrote {}", sig_path.display());

    // pubkey.hex for firmware const
    let pubkey_path = out_dir.join("pubkey.hex");
    fs::write(&pubkey_path, hex::encode(pubkey.as_bytes()))?;
    eprintln!("wrote {}", pubkey_path.display());

    Ok(())
}

fn build_payload(version: &str, hash: &[u8], n_chunks: usize) -> Vec<u8> {
    let mut payload = Vec::new();
    let vbytes = version.as_bytes();
    payload.extend_from_slice(&(vbytes.len() as u32).to_be_bytes());
    payload.extend_from_slice(vbytes);
    payload.extend_from_slice(hash);
    payload.extend_from_slice(&(n_chunks as u32).to_be_bytes());
    payload
}

fn verify_manifest(manifest_path: &std::path::Path, pubkey_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_json: serde_json::Value = serde_json::from_slice(&fs::read(manifest_path)?)?;
    let v = manifest_json.get("v").and_then(|x| x.as_str()).ok_or("manifest missing v")?;
    let h_hex = manifest_json.get("h").and_then(|x| x.as_str()).ok_or("manifest missing h")?;
    let n = manifest_json.get("n").and_then(|x| x.as_u64()).ok_or("manifest missing n")? as usize;
    let sig_b64 = manifest_json.get("sig").and_then(|x| x.as_str()).ok_or("manifest missing sig")?;

    let h = hex::decode(h_hex).map_err(|e| format!("hash hex: {}", e))?;
    if h.len() != 32 {
        return Err("hash must be 32 bytes".into());
    }
    let sig_bytes = B64.decode(sig_b64).map_err(|e| format!("sig base64: {}", e))?;
    if sig_bytes.len() != 64 {
        return Err("signature must be 64 bytes".into());
    }
    let sig = Signature::from_bytes(sig_bytes.as_slice().try_into().unwrap());

    let pubkey_hex = fs::read_to_string(pubkey_path)?.trim().to_string();
    let pubkey_bytes = hex::decode(&pubkey_hex).map_err(|e| format!("pubkey hex: {}", e))?;
    if pubkey_bytes.len() != 32 {
        return Err("pubkey must be 32 bytes".into());
    }
    let verifying_key = VerifyingKey::from_bytes(pubkey_bytes.as_slice().try_into().unwrap())?;

    let payload = build_payload(v, &h, n);
    verifying_key.verify(&payload, &sig).map_err(|_| "signature verification failed")?;
    eprintln!("OK manifest verified v={} n={}", v, n);
    Ok(())
}

fn read_file(p: &std::path::Path) -> io::Result<Vec<u8>> {
    let mut f = fs::File::open(p)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    Ok(buf)
}

fn load_signing_key(path: &std::path::Path) -> Result<SigningKey, Box<dyn std::error::Error>> {
    let data = fs::read(path)?;
    // If PEM
    if data.starts_with(b"-----") {
        let pem = std::str::from_utf8(&data)?;
        let der = pem::parse(pem).map_err(|e| format!("PEM parse: {}", e))?.contents;
        let key = ed25519_dalek::pkcs8::KeypairBytes::from_pkcs8_der(&der)
            .map_err(|e| format!("PKCS#8: {}", e))?;
        return Ok(SigningKey::from_bytes(&key.secret_key));
    }
    // Raw 32-byte seed hex
    let s = std::str::from_utf8(&data)?.trim();
    let bytes = hex::decode(s).map_err(|e| format!("hex decode: {}", e))?;
    if bytes.len() != 32 {
        return Err("key must be 32 bytes (hex = 64 chars)".into());
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(SigningKey::from_bytes(&arr))
}

// Minimal PEM parser for PKCS#8 private key (single base64 blob)
mod pem {
    use base64::Engine;
    pub struct Pem {
        pub contents: Vec<u8>,
    }
    pub fn parse(s: &str) -> Result<Pem, String> {
        let mut in_block = false;
        let mut b64 = String::new();
        for line in s.lines() {
            let line = line.trim();
            if line.starts_with("-----BEGIN") {
                in_block = true;
                continue;
            }
            if line.starts_with("-----END") {
                break;
            }
            if in_block {
                b64.push_str(line);
            }
        }
        let contents = base64::engine::general_purpose::STANDARD
            .decode(b64.as_bytes())
            .map_err(|e| e.to_string())?;
        Ok(Pem { contents })
    }
}
