//! Three-layer asset identification (techspec §9):
//!
//!   GUID (Uuid v4)  ←→  path_hash (u64 FNV-1a)  ←→  path (String)
//!
//! GUIDs are canonical and stable across renames/moves. Path hashes are the
//! fast runtime lookup key inside WPAK archives. Paths are human-readable and
//! may change.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use uuid::Uuid;

use crate::wpak;

// ============================================================================
// Load state
// ============================================================================

#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    Unloaded,
    Loading,
    Loaded,
    Failed(String),
}

// ============================================================================
// Data source — which archive or loose file holds the bytes
// ============================================================================

#[derive(Clone, Debug)]
pub enum DataSource {
    None,
    /// Index into `AssetSystem::archives`.
    Archive { archive_index: usize },
    Loose,
}

// ============================================================================
// AssetId — the three-layer identity
// ============================================================================

#[derive(Clone, Debug)]
pub struct AssetId {
    pub guid: Uuid,
    pub path_hash: u64,
    pub path: String,
}

impl AssetId {
    pub fn for_path(path: &str) -> Self {
        Self {
            guid: Uuid::new_v4(),
            path_hash: wpak::fnv1a(path),
            path: wpak::normalize_path(path),
        }
    }

    pub fn for_path_with_guid(path: &str, guid: Uuid) -> Self {
        Self {
            guid,
            path_hash: wpak::fnv1a(path),
            path: wpak::normalize_path(path),
        }
    }
}

// ============================================================================
// AssetRecord — tracks one loaded (or loading) asset
// ============================================================================

pub struct AssetRecord {
    pub id: AssetId,
    pub data_source: DataSource,
    pub ref_count: AtomicU32,
    pub load_state: LoadState,
    data: Option<Arc<Vec<u8>>>,
}

impl AssetRecord {
    pub fn new(id: AssetId, data_source: DataSource) -> Self {
        Self {
            id,
            data_source,
            ref_count: AtomicU32::new(0),
            load_state: LoadState::Unloaded,
            data: None,
        }
    }

    pub fn data(&self) -> Option<&Arc<Vec<u8>>> {
        self.data.as_ref()
    }

    pub fn set_data(&mut self, bytes: Arc<Vec<u8>>) {
        self.data = Some(bytes);
        self.load_state = LoadState::Loaded;
    }

    pub fn set_failed(&mut self, error: String) {
        self.load_state = LoadState::Failed(error);
    }

    pub fn acquire(&self) {
        self.ref_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn release(&self) -> u32 {
        self.ref_count.fetch_sub(1, Ordering::Relaxed).saturating_sub(1)
    }

    pub fn refs(&self) -> u32 {
        self.ref_count.load(Ordering::Relaxed)
    }
}

// ============================================================================
// AssetTable — dual-lookup cache by path_hash and GUID
// ============================================================================

pub struct AssetTable {
    records: HashMap<u64, Arc<AssetRecord>>,
    guid_index: HashMap<Uuid, u64>,
}

impl Clone for AssetTable {
    fn clone(&self) -> Self {
        Self {
            records: self.records.clone(),
            guid_index: self.guid_index.clone(),
        }
    }
}

impl AssetTable {
    pub fn new() -> Self {
        Self {
            records: HashMap::new(),
            guid_index: HashMap::new(),
        }
    }

    pub fn insert(&mut self, record: AssetRecord) -> Arc<AssetRecord> {
        let hash = record.id.path_hash;
        let guid = record.id.guid;
        let arc = Arc::new(record);
        self.guid_index.insert(guid, hash);
        self.records.insert(hash, Arc::clone(&arc));
        arc
    }

    pub fn by_hash(&self, path_hash: u64) -> Option<&Arc<AssetRecord>> {
        self.records.get(&path_hash)
    }

    pub fn by_guid(&self, guid: &Uuid) -> Option<&Arc<AssetRecord>> {
        let hash = self.guid_index.get(guid)?;
        self.records.get(hash)
    }

    pub fn by_path(&self, path: &str) -> Option<&Arc<AssetRecord>> {
        let hash = wpak::fnv1a(path);
        self.records.get(&hash)
    }

    pub fn contains_guid(&self, guid: &Uuid) -> bool {
        self.guid_index.contains_key(guid)
    }

    pub fn contains_hash(&self, path_hash: u64) -> bool {
        self.records.contains_key(&path_hash)
    }

    pub fn contains_path(&self, path: &str) -> bool {
        let hash = wpak::fnv1a(path);
        self.records.contains_key(&hash)
    }

    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn evict(&mut self, path_hash: u64) {
        if let Some(record) = self.records.remove(&path_hash) {
            self.guid_index.remove(&record.id.guid);
        }
    }

    /// Remove entries whose ref count has dropped to zero.
    pub fn evict_unreferenced(&mut self) {
        let to_evict: Vec<u64> = self
            .records
            .iter()
            .filter(|(_, r)| r.refs() == 0)
            .map(|(h, _)| *h)
            .collect();
        for hash in to_evict {
            self.evict(hash);
        }
    }
}

impl Default for AssetTable {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// .meta sidecar files — persistent GUID storage
// ============================================================================

/// Write a `.meta` sidecar file next to a source asset.
///
/// Format: single line containing the UUID v4 string.
pub fn write_meta(asset_path: &Path, guid: &Uuid) -> Result<(), String> {
    let meta_path = asset_path.with_extension("meta");
    std::fs::write(&meta_path, guid.to_string())
        .map_err(|e| format!("Failed to write meta '{}': {}", meta_path.display(), e))
}

/// Read GUID from a `.meta` sidecar file.
pub fn read_meta(asset_path: &Path) -> Result<Uuid, String> {
    let meta_path = asset_path.with_extension("meta");
    let content = std::fs::read_to_string(&meta_path)
        .map_err(|e| format!("Failed to read meta '{}': {}", meta_path.display(), e))?;
    Uuid::parse_str(content.trim())
        .map_err(|e| format!("Invalid GUID in '{}': {}", meta_path.display(), e))
}

/// Generate or load a GUID for the given asset path.
/// If a `.meta` sidecar exists, loads the GUID from it.
/// Otherwise generates a new UUID v4 and writes the `.meta` file.
pub fn guid_for_path(asset_path: &Path) -> Result<Uuid, String> {
    let meta_path = asset_path.with_extension("meta");
    if meta_path.exists() {
        read_meta(asset_path)
    } else {
        let guid = Uuid::new_v4();
        write_meta(asset_path, &guid)?;
        Ok(guid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_id_round_trip() {
        let id = AssetId::for_path("textures/player.png");
        assert_eq!(id.path, "textures/player.png");
        assert_eq!(id.path_hash, wpak::fnv1a("textures/player.png"));
        // UUID v4 variant should be 0b10xx (RFC 4122)
        assert_eq!(id.guid.get_version(), Some(uuid::Version::Random));
    }

    #[test]
    fn asset_table_insert_lookup() {
        let mut table = AssetTable::new();
        let id = AssetId::for_path("meshes/crate.wmesh");
        let record = AssetRecord::new(id.clone(), DataSource::Loose);
        let arc = table.insert(record);

        // Lookup by hash
        let by_hash = table.by_hash(id.path_hash).unwrap();
        assert_eq!(by_hash.id.path, "meshes/crate.wmesh");

        // Lookup by GUID
        let by_guid = table.by_guid(&id.guid).unwrap();
        assert_eq!(by_guid.id.path_hash, id.path_hash);

        // Lookup by path
        let by_path = table.by_path("MESHES\\Crate.wmesh").unwrap();
        assert_eq!(by_path.id.path_hash, id.path_hash);

        // Contains checks
        assert!(table.contains_path("meshes/crate.wmesh"));
        assert!(table.contains_hash(id.path_hash));
        assert!(table.contains_guid(&id.guid));
    }

    #[test]
    fn asset_record_ref_count() {
        let id = AssetId::for_path("test.dat");
        let record = AssetRecord::new(id, DataSource::None);
        assert_eq!(record.refs(), 0);
        record.acquire();
        record.acquire();
        assert_eq!(record.refs(), 2);
        record.release();
        assert_eq!(record.refs(), 1);
        record.release();
        assert_eq!(record.refs(), 0);
    }

    #[test]
    fn meta_file_round_trip() {
        let dir = std::env::temp_dir().join("waraner_meta_test");
        let _ = std::fs::create_dir_all(&dir);
        let asset_path = dir.join("test_asset.png");
        let guid = Uuid::new_v4();

        write_meta(&asset_path, &guid).expect("write meta");
        let loaded = read_meta(&asset_path).expect("read meta");
        assert_eq!(loaded, guid);

        let reloaded = guid_for_path(&asset_path).expect("guid for path");
        assert_eq!(reloaded, guid);

        let _ = std::fs::remove_file(asset_path.with_extension("meta"));
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn normalize_path_consistency() {
        let raw = "TEXTURES\\Player.PNG";
        let id = AssetId::for_path(raw);
        assert_eq!(id.path, "textures/player.png");
        assert_eq!(id.path_hash, wpak::fnv1a("textures/player.png"));
    }
}
