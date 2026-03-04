//! Flash formatting: CRC32, otadata entry layout, alignment helpers.
//!
//! These are pure functions that match the ESP-IDF bootloader's expectations.

/// CRC32 (IEEE 802.3 / zlib polynomial 0xEDB88320, same as ESP-IDF).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            if crc & 1 != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

// ---------------------------------------------------------------------------
// esp_ota_select_entry_t layout (32 bytes):
//   offset  0: ota_seq   (u32 LE) — 1-based; (seq-1) % num_ota selects partition
//   offset  4: seq_label (20 bytes, zeroed)
//   offset 24: ota_state (u32 LE) — 0 = ESP_OTA_IMG_NEW
//   offset 28: crc       (u32 LE) — CRC32 over bytes 0..27
// ---------------------------------------------------------------------------

/// OTA state: new image, boot normally.
pub const OTA_STATE_NEW: u32 = 0;

/// Build a 32-byte otadata entry that selects ota_0 (seq=1).
pub fn build_otadata_entry() -> [u8; 32] {
    build_otadata_entry_with_seq(1, OTA_STATE_NEW)
}

/// Build a 32-byte otadata entry with the given sequence number and state.
/// `seq=1` selects ota_0, `seq=2` selects ota_1, etc.
pub fn build_otadata_entry_with_seq(seq: u32, state: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[0..4].copy_from_slice(&seq.to_le_bytes());
    // seq_label: 20 bytes of zeros (already zero)
    entry[24..28].copy_from_slice(&state.to_le_bytes());
    // CRC32 over first 28 bytes
    let crc = crc32(&entry[..28]);
    entry[28..32].copy_from_slice(&crc.to_le_bytes());
    entry
}

/// Pad data to 4-byte alignment for flash writes. Trailing bytes are filled
/// with 0xFF (erased flash state). Returns a new Vec only if padding was needed.
pub fn pad_to_4_align(data: &[u8]) -> alloc::vec::Vec<u8> {
    let tail = data.len() % 4;
    if tail == 0 {
        return data.to_vec();
    }
    let mut padded = alloc::vec::Vec::with_capacity(data.len() + (4 - tail));
    padded.extend_from_slice(data);
    for _ in 0..(4 - tail) {
        padded.push(0xFF);
    }
    padded
}

// ---------------------------------------------------------------------------
// Flash storage trait — implemented by real ROM on device, MockFlash in tests
// ---------------------------------------------------------------------------

/// Abstract flash storage for OTA writes.
pub trait OtaFlash {
    type Error: core::fmt::Debug;

    /// Erase sectors covering `start_offset..start_offset+len`.
    /// Both values are byte offsets (not sector indices).
    fn erase(&mut self, start_offset: u32, len: u32) -> Result<(), Self::Error>;

    /// Write data at the given byte offset. Data must be 4-byte aligned in length
    /// (caller is responsible for padding).
    fn write(&mut self, offset: u32, data: &[u8]) -> Result<(), Self::Error>;

    /// Read data from the given byte offset.
    fn read(&self, offset: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
}

/// Write data to flash with automatic 4-byte alignment padding.
pub fn write_aligned<F: OtaFlash>(flash: &mut F, offset: u32, data: &[u8]) -> Result<(), F::Error> {
    let len = data.len();
    if len == 0 {
        return Ok(());
    }
    // Write full 4-byte-aligned portion
    let aligned_len = len & !3;
    if aligned_len > 0 {
        flash.write(offset, &data[..aligned_len])?;
    }
    // Handle trailing 1-3 bytes
    let tail = len - aligned_len;
    if tail > 0 {
        let mut buf = [0xFFu8; 4];
        buf[..tail].copy_from_slice(&data[aligned_len..]);
        flash.write(offset + aligned_len as u32, &buf)?;
    }
    Ok(())
}
