//! Mesh OTA: write verified image to OTA partition, set boot, reboot.
//! Partition layout must match partitions_ota.csv.
//!
//! Pure logic (CRC32, otadata format, alignment) lives in `hypha_ota::flash_fmt`.
//! This module provides the hardware-specific ROM flash calls.

#![cfg(feature = "mesh_ota")]

use esp_hal::rom::spiflash::{
    esp_rom_spiflash_erase_sector, esp_rom_spiflash_unlock, esp_rom_spiflash_write,
};
use esp_hal::system::software_reset;

const SECTOR_SIZE: u32 = 4096;

/// ota_0 partition start. With the default partition table layout:
///   nvs(0x4000) + otadata(0x2000) + phy_init(0x1000) = 0x7000 of data partitions.
///   factory app starts at 0x10000 (64KB aligned), size 1MB -> ends at 0x110000.
///   ota_0 starts at 0x110000.
const OTA_0_OFFSET: u32 = 0x110_000;
const OTA_0_SIZE: u32 = 0x100_000; // 1MB

/// otadata partition offset. nvs starts at 0x9000, size 0x4000 -> ends at 0xD000.
const OTADATA_OFFSET: u32 = 0xD_000;

/// Write a buffer to flash with automatic 4-byte alignment padding.
/// Returns true on success.
unsafe fn flash_write_aligned(offset: u32, data: &[u8]) -> bool {
    let len = data.len();
    if len == 0 {
        return true;
    }
    let aligned_len = len & !3;
    if aligned_len > 0 {
        let ptr = data.as_ptr() as *const u32;
        if unsafe { esp_rom_spiflash_write(offset, ptr, aligned_len as u32) } != 0 {
            return false;
        }
    }
    let tail = len - aligned_len;
    if tail > 0 {
        let mut buf = [0xFFu8; 4];
        buf[..tail].copy_from_slice(&data[aligned_len..]);
        let ptr = buf.as_ptr() as *const u32;
        if unsafe { esp_rom_spiflash_write(offset + aligned_len as u32, ptr, 4) } != 0 {
            return false;
        }
    }
    true
}

/// Erase ota_0 partition. Call once before streaming chunks.
pub fn erase_ota_partition(image_len: u32) -> bool {
    if image_len > OTA_0_SIZE || image_len == 0 {
        return false;
    }
    unsafe {
        if esp_rom_spiflash_unlock() != 0 {
            return false;
        }
        let start_sector = OTA_0_OFFSET / SECTOR_SIZE;
        let num_sectors = (image_len + SECTOR_SIZE - 1) / SECTOR_SIZE;
        for i in 0..num_sectors {
            if esp_rom_spiflash_erase_sector(start_sector + i) != 0 {
                return false;
            }
        }
    }
    true
}

/// Write a single chunk to ota_0 at the correct offset.
pub fn write_ota_chunk(chunk_index: u32, chunk_data: &[u8]) -> bool {
    let offset = OTA_0_OFFSET + chunk_index * hypha_ota::protocol::CHUNK_SIZE as u32;
    unsafe { flash_write_aligned(offset, chunk_data) }
}

/// Set otadata to boot ota_0 and reboot. Does not return on success.
pub fn set_boot_ota0_and_reboot() {
    // Use the shared pure-logic otadata builder from hypha-ota
    let entry = hypha_ota::flash_fmt::build_otadata_entry();
    unsafe {
        if esp_rom_spiflash_unlock() != 0 {
            return;
        }
        let otadata_sector = OTADATA_OFFSET / SECTOR_SIZE;
        if esp_rom_spiflash_erase_sector(otadata_sector) != 0 {
            return;
        }
        if !flash_write_aligned(OTADATA_OFFSET, &entry) {
            return;
        }
        // Erase second sector so bootloader doesn't pick stale entry
        let otadata_sector2 = (OTADATA_OFFSET + SECTOR_SIZE) / SECTOR_SIZE;
        if esp_rom_spiflash_erase_sector(otadata_sector2) != 0 {
            return;
        }
    }
    software_reset();
}

/// Legacy: write full image at once (for small images that fit in RAM).
pub fn write_ota_partition_and_reboot(image: &[u8]) {
    let image_len = image.len() as u32;
    if !erase_ota_partition(image_len) {
        return;
    }
    unsafe {
        if !flash_write_aligned(OTA_0_OFFSET, image) {
            return;
        }
    }
    set_boot_ota0_and_reboot();
}
