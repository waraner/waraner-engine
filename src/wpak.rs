//! WPAK — packed resource archive (techspec §2).
//!
//! Read-only archive of all runtime assets. The FILE INDEX is sorted by
//! `path_hash` (FNV-1a over the forward-slash-normalized UTF-8 path) and
//! binary-searched on lookup. Each entry may carry compressed payloads
//! (Zstd / LZ4 / deflate / none); the layout is wire-compatible with the
//! asset pipeline described in §14.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use uuid::Uuid;

// --- Binary layout constants (must match techspec §2) ----------------------

const WPAK_MAGIC: [u8; 4] = *b"WPAK";
const CURRENT_VERSION: u32 = 2;

const HEADER_SIZE: usize = 4 + 4 + 4 + 8 + 8 + 8 + 8 + 8; // 52 bytes (magic, version, flags + 6× u64)

// Header field offsets
const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_FLAGS: usize = 8;
const OFF_FILE_COUNT: usize = 12;
const OFF_INDEX_OFFSET: usize = 20;
const OFF_INDEX_SIZE: usize = 28;
const OFF_DATA_OFFSET: usize = 36;
const OFF_GUID_OFFSET: usize = 44;

const FLAG_HAS_GUID_TABLE: u32 = 1 << 0;

// FileEntry field offsets (within each 48-byte record)
const ENT_PATH_HASH: usize = 0;
const ENT_PATH_OFFSET: usize = 8;
const ENT_DATA_OFFSET: usize = 16;
const ENT_DATA_SIZE: usize = 24;
const ENT_COMPRESSED_SZ: usize = 32;
const ENT_COMPRESSION: usize = 40;
// + 7 bytes padding -> 48

const FILE_ENTRY_SIZE: usize = 48;
const NAME_TERMINATOR: u8 = 0;

// --- Compression -----------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    None = 0,
    Zstd = 1,
    Lz4 = 2,
    Deflate = 3,
}

impl Compression {
    fn from_id(id: u8) -> Compression {
        match id {
            1 => Compression::Zstd,
            2 => Compression::Lz4,
            3 => Compression::Deflate,
            _ => Compression::None,
        }
    }

    pub fn id(self) -> u8 {
        self as u8
    }
}

fn compress(raw: &[u8], algo: Compression) -> Result<Vec<u8>, String> {
    match algo {
        Compression::None => Ok(raw.to_vec()),
        Compression::Zstd => zstd::encode_all(raw, 3)
            .map_err(|e| format!("zstd compression failed: {e}")),
        Compression::Lz4 => {
            let mut buf = Vec::new();
            let mut encoder = lz4::EncoderBuilder::new()
                .build(&mut buf)
                .map_err(|e| format!("lz4 compression failed: {e}"))?;
            std::io::Write::write_all(&mut encoder, raw)
                .map_err(|e| format!("lz4 write failed: {e}"))?;
            encoder
                .finish()
                .1
                .map_err(|e| format!("lz4 finish failed: {e}"))?;
            Ok(buf)
        }
        Compression::Deflate => {
            let mut buf = Vec::new();
            let mut encoder =
                flate2::write::ZlibEncoder::new(&mut buf, flate2::Compression::default());
            std::io::Write::write_all(&mut encoder, raw)
                .map_err(|e| format!("deflate compression failed: {e}"))?;
            encoder
                .finish()
                .map_err(|e| format!("deflate finish failed: {e}"))?;
            Ok(buf)
        }
    }
}

fn decompress(compressed: &[u8], algo: Compression, raw_size: u64) -> Result<Vec<u8>, String> {
    match algo {
        Compression::None => Ok(compressed.to_vec()),
        Compression::Zstd => zstd::decode_all(compressed)
            .map_err(|e| format!("zstd decompression failed: {e}")),
        Compression::Lz4 => {
            let mut decoder = lz4::Decoder::new(compressed)
                .map_err(|e| format!("lz4 decode init failed: {e}"))?;
            let mut out = Vec::with_capacity(raw_size as usize);
            std::io::Read::read_to_end(&mut decoder, &mut out)
                .map_err(|e| format!("lz4 decode failed: {e}"))?;
            Ok(out)
        }
        Compression::Deflate => {
            let mut decoder = flate2::read::ZlibDecoder::new(compressed);
            let mut out = Vec::with_capacity(raw_size as usize);
            std::io::Read::read_to_end(&mut decoder, &mut out)
                .map_err(|e| format!("deflate decode failed: {e}"))?;
            Ok(out)
        }
    }
}

// --- FNV-1a 64-bit ---------------------------------------------------------

pub fn fnv1a(path: &str) -> u64 {
    let normalized = normalize_path(path);
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    for &byte in normalized.as_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// Normalize a path to forward slashes, trimmed of a leading slash and any
/// drive/authority prefixes, lowercased (case-insensitive on most filesystems).
pub fn normalize_path(path: &str) -> String {
    let mut s = path.replace('\\', "/");
    if let Some(idx) = s.find("://") {
        // strip scheme like file://
        s = s[idx + 3..].to_string();
    }
    if s.starts_with('/') {
        s = s[1..].to_string();
    }
    s.to_lowercase()
}

// --- FileEntry (in-memory) -------------------------------------------------

#[derive(Clone, Debug)]
pub struct FileEntry {
    pub path: String,
    pub path_hash: u64,
    pub data: Vec<u8>,
    pub compression: Compression,
    /// Optional UUID v4 — stable canonical identity.
    pub guid: Option<Uuid>,
}

impl FileEntry {
    fn compressed(&self) -> Vec<u8> {
        compress(&self.data, self.compression).expect("compression")
    }
}

// --- Reader -----------------------------------------------------------------

pub struct WpakArchive {
    data: Arc<Vec<u8>>,
    file_count: u64,
    index_offset: u64,
    data_offset: u64,
    // path_hash -> byte offset of the FileEntry within the index
    hash_index: HashMap<u64, u64>,
    // path_hash -> original path (for collision recovery / tooling)
    paths: HashMap<u64, String>,
    // path_hash -> optional GUID (UUID v4)
    guids: HashMap<u64, Uuid>,
}

impl WpakArchive {
    pub fn open(path: &str) -> Result<Self, String> {
        let bytes = fs::read(path).map_err(|e| format!("Failed to read WPAK '{path}': {e}"))?;
        Self::from_bytes(Arc::new(bytes))
    }

    pub fn from_bytes(bytes: Arc<Vec<u8>>) -> Result<Self, String> {
        if bytes.len() < HEADER_SIZE {
            return Err("WPAK too small to contain header".to_string());
        }
        let magic = &bytes[OFF_MAGIC..OFF_MAGIC + 4];
        if magic != WPAK_MAGIC {
            return Err(format!(
                "Invalid WPAK magic: expected WPAK, got {:?}",
                std::str::from_utf8(magic).unwrap_or("???")
            ));
        }
        let version = read_u32(&bytes, OFF_VERSION);
        if version > CURRENT_VERSION {
            return Err(format!(
                "Unsupported WPAK version {} (expected {})",
                version, CURRENT_VERSION
            ));
        }
        if version < CURRENT_VERSION - 1 {
            // Too old to read. We support v1 (CURRENT-1) and v2.
            return Err(format!(
                "WPAK version {} too old (need at least {})",
                version,
                CURRENT_VERSION - 1
            ));
        }
        let flags = read_u32(&bytes, OFF_FLAGS);
        let file_count = read_u64(&bytes, OFF_FILE_COUNT);
        let index_offset = read_u64(&bytes, OFF_INDEX_OFFSET);
        let _index_size = read_u64(&bytes, OFF_INDEX_SIZE);
        let data_offset = read_u64(&bytes, OFF_DATA_OFFSET);
        let guid_offset = if version >= 2 {
            read_u64(&bytes, OFF_GUID_OFFSET)
        } else {
            0
        };

        let mut hash_index = HashMap::new();
        let mut paths = HashMap::new();
        let mut guids = HashMap::new();

        for i in 0..file_count {
            let base = index_offset as usize + (i * FILE_ENTRY_SIZE as u64) as usize;
            if base + FILE_ENTRY_SIZE > bytes.len() {
                return Err("WPAK index entry out of range".to_string());
            }
            let path_hash = read_u64(&bytes, base + ENT_PATH_HASH);
            let path_offset = read_u64(&bytes, base + ENT_PATH_OFFSET) as usize;
            let path_end = bytes[path_offset..]
                .iter()
                .position(|&b| b == NAME_TERMINATOR)
                .map(|p| path_offset + p)
                .ok_or_else(|| "WPAK entry name not null-terminated".to_string())?;
            let path = String::from_utf8(bytes[path_offset..path_end].to_vec())
                .map_err(|_| "WPAK entry name is not valid UTF-8".to_string())?;

            hash_index.insert(path_hash, base as u64);
            paths.insert(path_hash, path);

            // Read optional GUID from the GUID table (version 2+).
            if guid_offset > 0 && (flags & FLAG_HAS_GUID_TABLE) != 0 {
                let guid_base = guid_offset as usize + (i * 16) as usize;
                if guid_base + 16 <= bytes.len() {
                    let mut guid_bytes = [0u8; 16];
                    guid_bytes.copy_from_slice(&bytes[guid_base..guid_base + 16]);
                    let guid = Uuid::from_bytes(guid_bytes);
                    if !guid.is_nil() {
                        guids.insert(path_hash, guid);
                    }
                }
            }
        }

        Ok(Self {
            data: bytes,
            file_count,
            index_offset,
            data_offset,
            hash_index,
            paths,
            guids,
        })
    }

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    /// Resolve an asset path to its stored path (may differ in casing/separators).
    pub fn resolve_path(&self, path: &str) -> Option<String> {
        let target = fnv1a(path);
        self.paths.get(&target).cloned()
    }

    /// Look up the GUID for a given path, if one exists in the archive.
    pub fn resolve_guid(&self, path: &str) -> Option<Uuid> {
        let target = fnv1a(path);
        self.guids.get(&target).copied()
    }

    /// Look up the path for a given GUID, if present.
    pub fn resolve_guid_path(&self, guid: &Uuid) -> Option<String> {
        for (&hash, stored_guid) in &self.guids {
            if stored_guid == guid {
                return self.paths.get(&hash).cloned();
            }
        }
        None
    }

    /// Iterate all (path_hash, GUID) pairs stored in this archive.
    pub fn all_guids(&self) -> &HashMap<u64, Uuid> {
        &self.guids
    }

    /// Read (and decompress) the raw bytes for an asset path.
    pub fn read(&self, path: &str) -> Result<Arc<Vec<u8>>, String> {
        let target = fnv1a(path);
        let base = *self
            .hash_index
            .get(&target)
            .ok_or_else(|| format!("Asset '{}' not found in archive", path))?;

        let base = base as usize;
        let data_offset = read_u64(&self.data, base + ENT_DATA_OFFSET);
        let data_size = read_u64(&self.data, base + ENT_DATA_SIZE) as usize;
        let compressed_sz = read_u64(&self.data, base + ENT_COMPRESSED_SZ) as usize;
        let compression = Compression::from_id(self.data[base + ENT_COMPRESSION]);

        let start = (self.data_offset + data_offset) as usize;
        let raw = if compression == Compression::None {
            &self.data[start..start + data_size]
        } else {
            &self.data[start..start + compressed_sz]
        };

        let out = decompress(raw, compression, data_size as u64)?;
        Ok(Arc::new(out))
    }
}

// --- Builder ----------------------------------------------------------------

/// Build a `.wpak` archive from a set of (path, bytes) entries.
pub fn build_archive(entries: &[FileEntry], path: &str) -> Result<(), String> {
    // Sort by path_hash (binary-searchable index per spec).
    let mut sorted = entries.to_vec();
    sorted.sort_by_key(|e| e.path_hash);

    // Index section: 48-byte FileEntry records + null-terminated path strings.
    let total_index_size = sorted.len() * FILE_ENTRY_SIZE;
    let mut index: Vec<u8> = Vec::with_capacity(total_index_size);
    let mut name_blob: Vec<u8> = Vec::new();
    // Per-entry data offset (relative to data section start), patched later.
    let mut data_offsets: Vec<u64> = Vec::with_capacity(sorted.len());

    let mut running_offset: u64 = 0;
    for entry in &sorted {
        let path_offset = (HEADER_SIZE + total_index_size + name_blob.len()) as u64;
        let compressed = entry.compressed();
        let stored_size = entry.data.len() as u64;
        let (compressed_sz, payload_len) = if entry.compression == Compression::None {
            (0u64, entry.data.len() as u64)
        } else {
            (compressed.len() as u64, compressed.len() as u64)
        };

        index.extend_from_slice(&entry.path_hash.to_le_bytes());
        index.extend_from_slice(&path_offset.to_le_bytes());
        index.extend_from_slice(&0u64.to_le_bytes()); // data_offset (patched later)
        index.extend_from_slice(&stored_size.to_le_bytes());
        index.extend_from_slice(&compressed_sz.to_le_bytes());
        index.extend_from_slice(&entry.compression.id().to_le_bytes());
        index.extend_from_slice(&[0u8; 7]); // padding

        name_blob.extend_from_slice(entry.path.as_bytes());
        name_blob.push(NAME_TERMINATOR);

        data_offsets.push(running_offset);
        running_offset += payload_len;
        // 16-byte alignment of the next entry within the data section.
        running_offset += (16 - (running_offset % 16)) % 16;
    }

    // Data section offset (after header + index + names).
    let data_start = HEADER_SIZE + index.len() + name_blob.len();

    // Recompute data_offsets using absolute-position-aware padding (must
    // match the data-writing loop below which uses out.len()).
    {
        let mut abs_pos = data_start as u64;
        for (i, entry) in sorted.iter().enumerate() {
            let payload_len = if entry.compression == Compression::None {
                entry.data.len() as u64
            } else {
                entry.compressed().len() as u64
            };
            data_offsets[i] = abs_pos - data_start as u64;
            abs_pos += payload_len;
            abs_pos += (16 - (abs_pos % 16)) % 16;
        }
    }

    // Compute total payload + alignment padding to size the buffer.
    let mut total = data_start;
    for entry in &sorted {
        let payload_len = if entry.compression == Compression::None {
            entry.data.len()
        } else {
            entry.compressed().len()
        };
        total += payload_len;
        total += (16 - (total % 16)) % 16;
    }

    let mut out = Vec::with_capacity(total);
    // Header placeholder (40 bytes).
    out.extend(std::iter::repeat(0u8).take(HEADER_SIZE));
    out.extend_from_slice(&index);
    out.extend_from_slice(&name_blob);
    // Pad up to the data section start.
    while out.len() < data_start {
        out.push(0);
    }

    for entry in sorted.iter() {
        let compressed = entry.compressed();
        let payload = if entry.compression == Compression::None {
            &entry.data
        } else {
            &compressed
        };
        out.extend_from_slice(payload);
        let pad = (16 - (out.len() % 16)) % 16;
        out.extend(std::iter::repeat(0u8).take(pad));
    }

    // Patch each FileEntry's data_offset.
    for (i, _entry) in sorted.iter().enumerate() {
        let base = HEADER_SIZE + i * FILE_ENTRY_SIZE;
        out[base + ENT_DATA_OFFSET..base + ENT_DATA_OFFSET + 8]
            .copy_from_slice(&data_offsets[i].to_le_bytes());
    }

    // Optional GUID table (written after data section, 16-byte aligned).
    let has_guids = sorted.iter().any(|e| e.guid.is_some());
    if has_guids {
        while out.len() % 16 != 0 {
            out.push(0);
        }
        let guid_offset = out.len() as u64;
        for entry in &sorted {
            let guid_bytes = entry.guid.unwrap_or(Uuid::nil()).to_bytes_le();
            out.extend_from_slice(&guid_bytes);
        }

        // Write the header with GUID offset.
        let index_offset = HEADER_SIZE as u64;
        let index_size = index.len() as u64;
        let data_offset = data_start as u64;

        out[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&WPAK_MAGIC);
        out[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&CURRENT_VERSION.to_le_bytes());
        out[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&FLAG_HAS_GUID_TABLE.to_le_bytes());
        out[OFF_FILE_COUNT..OFF_FILE_COUNT + 8].copy_from_slice(&(sorted.len() as u64).to_le_bytes());
        out[OFF_INDEX_OFFSET..OFF_INDEX_OFFSET + 8].copy_from_slice(&index_offset.to_le_bytes());
        out[OFF_INDEX_SIZE..OFF_INDEX_SIZE + 8].copy_from_slice(&index_size.to_le_bytes());
        out[OFF_DATA_OFFSET..OFF_DATA_OFFSET + 8].copy_from_slice(&data_offset.to_le_bytes());
        out[OFF_GUID_OFFSET..OFF_GUID_OFFSET + 8].copy_from_slice(&guid_offset.to_le_bytes());
    } else {
        // No GUIDs — write a clean v2 header with guid_offset = 0.
        let index_offset = HEADER_SIZE as u64;
        let index_size = index.len() as u64;
        let data_offset = data_start as u64;

        out[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&WPAK_MAGIC);
        out[OFF_VERSION..OFF_VERSION + 4].copy_from_slice(&CURRENT_VERSION.to_le_bytes());
        out[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&0u32.to_le_bytes());
        out[OFF_FILE_COUNT..OFF_FILE_COUNT + 8].copy_from_slice(&(sorted.len() as u64).to_le_bytes());
        out[OFF_INDEX_OFFSET..OFF_INDEX_OFFSET + 8].copy_from_slice(&index_offset.to_le_bytes());
        out[OFF_INDEX_SIZE..OFF_INDEX_SIZE + 8].copy_from_slice(&index_size.to_le_bytes());
        out[OFF_DATA_OFFSET..OFF_DATA_OFFSET + 8].copy_from_slice(&data_offset.to_le_bytes());
        out[OFF_GUID_OFFSET..OFF_GUID_OFFSET + 8].copy_from_slice(&0u64.to_le_bytes());
    }

    let mut file = fs::File::create(path).map_err(|e| format!("Failed to create '{path}': {e}"))?;
    file.write_all(&out)
        .map_err(|e| format!("Failed to write '{path}': {e}"))?;
    Ok(())
}

// --- Internal little-endian readers ----------------------------------------

fn read_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

fn read_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

// --- Convenience: pack a directory tree ------------------------------------

/// Recursively collect `(normalized_path, bytes)` for every file under `dir`.
pub fn collect_dir(dir: &Path) -> Result<Vec<FileEntry>, String> {
    let mut out = Vec::new();
    collect_dir_rec(dir, dir, &mut out)?;
    Ok(out)
}

fn collect_dir_rec(root: &Path, dir: &Path, out: &mut Vec<FileEntry>) -> Result<(), String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("Failed to read dir '{dir:?}': {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_dir() {
            collect_dir_rec(root, &path, out)?;
        } else {
            let rel = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            let data = fs::read(&path).map_err(|e| format!("Failed to read '{path:?}': {e}"))?;
            out.push(FileEntry {
                path: rel.clone(),
                path_hash: fnv1a(&rel),
                data,
                compression: Compression::None,
                guid: None,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, data: &[u8], compression: Compression) -> FileEntry {
        FileEntry {
            path: name.to_string(),
            path_hash: fnv1a(name),
            data: data.to_vec(),
            compression,
            guid: None,
        }
    }

    fn round_trip(entries: &[FileEntry]) {
        let path = std::env::temp_dir().join("waraner_test.wpak");
        let path_str = path.to_str().unwrap();
        build_archive(entries, path_str).expect("build");

        let arc = WpakArchive::open(path_str).expect("open");
        assert_eq!(arc.file_count(), entries.len() as u64);

        for e in entries {
            let got = arc.read(&e.path).expect(&format!("read {}", e.path));
            assert_eq!(&got[..], &e.data[..], "payload mismatch for {}", e.path);
            assert_eq!(arc.resolve_path(&e.path).as_deref(), Some(e.path.as_str()));
        }
        let _ = fs::remove_file(path_str);
    }

    #[test]
    fn fnv1a_is_stable() {
        // Case + separator insensitive.
        assert_eq!(fnv1a("textures/player.png"), fnv1a("TEXTURES\\player.PNG"));
    }

    #[test]
    fn round_trip_uncompressed() {
        round_trip(&[
            entry("textures/player.png", &[1, 2, 3, 4, 5], Compression::None),
            entry("models/teapot.wmesh", &[9, 8, 7], Compression::None),
        ]);
    }

    #[test]
    fn round_trip_all_compressions() {
        let payload = vec![42u8; 2048];
        round_trip(&[
            entry("a/zstd.bin", &payload, Compression::Zstd),
            entry("a/lz4.bin", &payload, Compression::Lz4),
            entry("a/deflate.bin", &payload, Compression::Deflate),
            entry("a/none.bin", &payload, Compression::None),
        ]);
    }

    #[test]
    fn missing_asset_errors() {
        let path = std::env::temp_dir().join("waraner_missing.wpak");
        let path_str = path.to_str().unwrap();
        build_archive(&[entry("exists.bin", &[1], Compression::None)], path_str).unwrap();
        let arc = WpakArchive::open(path_str).unwrap();
        assert!(arc.read("nope.bin").is_err());
        let _ = fs::remove_file(path_str);
    }
}
