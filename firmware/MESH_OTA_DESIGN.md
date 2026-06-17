# Mesh OTA: Secure peer-to-peer firmware updates

**Goal:** Update one device (via USB or HTTP OTA); its peers on the ESP-NOW mesh automatically pull the same firmware from it and apply it, after verifying a signature. No need to plug in or re-flash the other devices.

## Flow

1. **Source:** One device gets the new image:
   - **Option A:** Flashed via USB (current `just esp-c6-flash-all` on one port). That device then needs to "serve" the image — e.g. host pushes the binary over serial once after flash, device stores it and advertises to peers.
   - **Option B:** Device runs OTA firmware (idf), gets update from HTTP OTA server, reboots. We add ESP-NOW to that build so after boot it advertises "I have version X" and serves chunks to mesh peers.

2. **Advertise:** Source periodically broadcasts a **manifest** over ESP-NOW: version string, image hash (e.g. SHA-256), total chunk count, and a **signature** over (version | hash | chunk_count). Payload size fits in one ESP-NOW frame (~250 bytes).

3. **Request:** A peer that has an older version (or missing version) sends a **chunk request** to the source’s MAC (unicast): chunk index 0..N-1. Source replies with **chunk response**: index + payload (e.g. 200 bytes per chunk). Repeat until peer has full image.

4. **Verify:** Peer assembles the image in RAM or streams to a staging partition. Before committing, peer **verifies the signature** with the built-in public key. Only if verification passes does it write to the OTA partition and reboot.

5. **Secure:** All devices are built with the same **public key** (e.g. Ed25519). The build pipeline **signs** the firmware image with the corresponding private key (CI or host). Devices never accept an image that doesn’t verify.

## Security model

- **Integrity + authenticity:** Signature ensures the image is from a build that has the private key (you or your CI). Peers don’t trust "a friend" — they trust "whoever signed this image."
- **No secrets on device:** Only the public key is on the device; private key stays on build host or CI.
- **Optional:** Add a version or timestamp in the signed payload so devices only accept "newer" than current.

## Protocol (wire format)

- **Manifest (broadcast):** `{"ota":"manifest","v":"1.2.3","h":"<sha256 hex>","n":1234,"sig":"<base64>"}`. All fields covered by `sig`.
- **Chunk request (unicast to source):** `{"ota":"req","i":0}` (i = chunk index).
- **Chunk response (unicast to requester):** `{"ota":"chunk","i":0,"b":"<base64>"}`. Chunks are fixed size (e.g. 200 bytes) except the last.

## Implementation phases

| Phase | What | Where | Status |
|-------|------|--------|--------|
| 1 | Signing pipeline: build produces `firmware.bin` + manifest + `.sig`; embed public key in firmware | Host: `firmware/mesh_ota/` (mesh_ota_sign); `hypha_esp_c6` feature `mesh_ota` + build.rs | **Done** |
| 2 | OTA partition table + bootloader for esp-hal build; receive chunks, verify sig, write to partition, reboot | `hypha_esp_c6` (partition table, OTA receive, ota_apply) | **Partial** |
| 3 | Manifest broadcast + chunk server (sender) | Either (a) push image to one device over serial and it serves, or (b) idf build with ESP-NOW that serves after HTTP OTA | **Partial** |
| 4 | Receiver logic: listen for manifest, compare version, request chunks, verify, apply | `hypha_esp_c6` | **Partial** (RAM cap 256 chunks; full image needs stream-to-flash) |

**Phase 4 (partial):** Receiver: on OTA_VERIFIED, if n_chunks <= 256 (MAX_CHUNKS_IN_RAM), device requests chunks from the manifest sender, assembles in RAM, verifies image hash, prints OTA_READY, then writes image to ota_0, sets otadata, and reboots (Phase 2b). If n > 256 it prints "OTA n=... > MAX_CHUNKS_IN_RAM, need stream-to-flash". Full-image stream-to-flash is still open.

**Phase 3 (partial):** Sender: build with `MESH_OTA_MANIFEST_PATH=build/mesh_ota/manifest.json` (and pubkey) embeds manifest. Device broadcasts manifest every 30 s and responds to `{"ota":"req","i":N}` with `{"ota":"chunk","i":N,"b":"<base64>"}`. Chunk payload is currently stubbed; flash read for real chunks is TODO.

**Phase 2 (partial):** Partition table `firmware/hypha_esp_c6/partitions_ota.csv` (nvs, otadata, phy_init, factory, ota_0, ota_1). Use with `espflash flash --partition-table partitions_ota.csv --bootloader <path>` when an OTA-capable bootloader is used. In-firmware: manifest verification on RX (feature `mesh_ota`); on OTA_READY, `ota_apply::write_ota_partition_and_reboot` writes image to ota_0 via ROM spiflash, writes otadata (boot ota_0), and calls `software_reset`. End-to-end device update is not yet proven.

**Phase 1 (done):** From repo root: `just mesh-ota-keygen` (once), then `just mesh-ota-build` (builds firmware, saves image, signs; writes `manifest.json`, `firmware.sig`, `pubkey.hex` to `firmware/hypha_esp_c6/build/mesh_ota/`). `just mesh-ota-verify` verifies the manifest. To embed the pubkey in firmware: `MESH_OTA_PUBKEY_PATH=build/mesh_ota/pubkey.hex cargo build --release --features "led,mesh_ota"` from `firmware/hypha_esp_c6`, or `just mesh-ota-firmware`. Host test: `cargo test --manifest-path firmware/mesh_ota/Cargo.toml --test sign_verify`.

## Constraints

- **ESP-NOW payload:** ~250 bytes per frame. Chunk payloads therefore small; many round-trips for a ~500 KiB image. Acceptable for occasional updates.
- **RAM:** Receiver may need a staging buffer or stream-to-flash. ESP32-C6 has enough for a chunk buffer; full-image verify-then-write is possible if we stream to partition and verify at end.
- **Bootloader:** esp-hal build currently uses a single factory partition. Need a partition table with `ota_0` / `ota_1` and a bootloader that selects the next OTA slot (or use existing esp-idf bootloader behavior if we supply the table).

## Relation to current OTA

- **Current:** `hypha_esp_c6_idf` does HTTP OTA only (WiFi STA, no ESP-NOW). One device updated from server; others don’t get it from peers.
- **After mesh OTA:** Either the same esp-hal firmware (`hypha_esp_c6`) gets OTA partition + mesh OTA, or the idf build gains ESP-NOW and serves after HTTP OTA. Preferred for "one codebase" is to add OTA + mesh OTA to `hypha_esp_c6` so the same image does ESP-NOW mesh and can receive/serve firmware.

---

*This doc is the design; implementation is phased as above.*
