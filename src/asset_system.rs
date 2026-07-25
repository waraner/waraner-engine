//! Asset system — manages open WPAK archives and loose-file fallback
//! (techspec §2 / §9). Resolves assets by path, path_hash, or GUID, and
//! caches them in an `AssetTable` with reference counting.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use uuid::Uuid;

use crate::asset_id::{AssetId, AssetRecord, AssetTable, DataSource};
use crate::wpak::{Compression, FileEntry, WpakArchive};

/// Default archive filenames, searched in order (frequently-accessed first).
pub const DEFAULT_ARCHIVES: &[&str] = &[
    "pak_0000.wpak", // textures, shaders
    "pak_0001.wpak", // audio
    "pak_0002.wpak", // meshes
    "pak_0003.wpak", // ui / fonts
];

#[derive(Clone)]
pub struct AssetSystem {
    archives: Vec<Arc<WpakArchive>>,
    table: AssetTable,
    loose_dir: Option<PathBuf>,
    compression: Compression,
}

impl AssetSystem {
    pub fn new() -> Self {
        Self {
            archives: Vec::new(),
            table: AssetTable::new(),
            loose_dir: None,
            compression: Compression::None,
        }
    }

    /// Open every archive named in `archives` inside `data_dir/resources`.
    pub fn open_archives(data_dir: &Path, archives: &[&str]) -> Self {
        let mut system = Self::new();
        let resources = data_dir.join("resources");
        for name in archives {
            let path = resources.join(name);
            if let Ok(arc) = WpakArchive::open(path.to_str().unwrap_or(name)) {
                log::info!(
                    "Opened asset archive '{}' ({} files)",
                    path.display(),
                    arc.file_count()
                );
                system.archives.push(Arc::new(arc));
            } else {
                log::debug!("Asset archive '{}' not found (skipped)", path.display());
            }
        }
        if resources.exists() {
            system.loose_dir = Some(resources);
        }
        system
    }

    pub fn add_archive(&mut self, archive: WpakArchive) {
        self.archives.push(Arc::new(archive));
    }

    pub fn set_compression(&mut self, compression: Compression) {
        self.compression = compression;
    }

    // ------------------------------------------------------------------
    // Asset table access
    // ------------------------------------------------------------------

    pub fn table(&self) -> &AssetTable {
        &self.table
    }

    pub fn table_mut(&mut self) -> &mut AssetTable {
        &mut self.table
    }

    // ------------------------------------------------------------------
    // Load by path
    // ------------------------------------------------------------------

    /// Resolve an asset path to raw bytes.
    pub fn get_bytes(&mut self, path: &str) -> Result<Arc<Vec<u8>>, String> {
        let normalized = crate::wpak::normalize_path(path);

        if let Some(record) = self.table.by_path(&normalized) {
            if let Some(data) = record.data() {
                return Ok(Arc::clone(data));
            }
        }

        let data_source = self.find_archive_source(&normalized);
        let archive_idx = match data_source {
            Some(idx) => idx,
            None => {
                // Loose-file fallback.
                if let Some(dir) = &self.loose_dir {
                    let candidate = dir.join(&normalized);
                    if candidate.exists() {
                        let bytes = std::fs::read(&candidate)
                            .map_err(|e| format!("Failed to read '{}': {}", candidate.display(), e))?;
                        let bytes = Arc::new(bytes);
                        let id = AssetId::for_path(&normalized);
                        let mut record = AssetRecord::new(id, DataSource::Loose);
                        record.set_data(Arc::clone(&bytes));
                        self.table.insert(record);
                        return Ok(bytes);
                    }
                }
                return Err(format!("Asset '{}' not found in any archive or loose path", path));
            }
        };

        let arc = &self.archives[archive_idx];
        let bytes = arc.read(&normalized)?;
        let guid = arc.resolve_guid(&normalized);

        let id = match guid {
            Some(g) => AssetId::for_path_with_guid(&normalized, g),
            None => AssetId::for_path(&normalized),
        };
        let mut record = AssetRecord::new(id, DataSource::Archive { archive_index: archive_idx });
        record.set_data(Arc::clone(&bytes));
        record.acquire();
        self.table.insert(record);

        Ok(bytes)
    }

    /// True if the path can be resolved (without loading/caching the payload).
    pub fn contains(&self, path: &str) -> bool {
        let normalized = crate::wpak::normalize_path(path);
        self.table.contains_path(&normalized)
            || self.archives.iter().any(|a| a.resolve_path(&normalized).is_some())
    }

    // ------------------------------------------------------------------
    // Load by GUID
    // ------------------------------------------------------------------

    /// Resolve an asset by its UUID v4 GUID.
    pub fn get_bytes_by_guid(&mut self, guid: &Uuid) -> Result<Arc<Vec<u8>>, String> {
        if let Some(record) = self.table.by_guid(guid) {
            if let Some(data) = record.data() {
                return Ok(Arc::clone(data));
            }
        }

        // Search archives for the GUID to resolve its path.
        for (idx, arc) in self.archives.iter().enumerate() {
            if let Some(path) = arc.resolve_guid_path(guid) {
                let bytes = arc.read(&path)?;
                let id = AssetId::for_path_with_guid(&path, *guid);
                let mut record = AssetRecord::new(id, DataSource::Archive { archive_index: idx });
                record.set_data(Arc::clone(&bytes));
                record.acquire();
                self.table.insert(record);
                return Ok(bytes);
            }
        }

        Err(format!("Asset with GUID '{}' not found in any archive", guid))
    }

    /// Resolve path for a GUID from archives.
    pub fn resolve_guid_path(&self, guid: &Uuid) -> Option<String> {
        for arc in &self.archives {
            if let Some(path) = arc.resolve_guid_path(guid) {
                return Some(path);
            }
        }
        None
    }

    // ------------------------------------------------------------------
    // GUID discovery in archives
    // ------------------------------------------------------------------

    /// Iterate all (path_hash, GUID) pairs across all open archives.
    pub fn all_guid_pairs(&self) -> HashMap<u64, Uuid> {
        let mut out = HashMap::new();
        for arc in &self.archives {
            for (hash, guid) in arc.all_guids() {
                out.entry(*hash).or_insert(*guid);
            }
        }
        out
    }

    // ------------------------------------------------------------------
    // Packing
    // ------------------------------------------------------------------

    /// Pack a loose directory tree into a `.wpak` under `data_dir/resources`.
    /// GUIDs are loaded from .meta sidecar files if available.
    pub fn pack_dir(&self, dir: &Path, out_name: &str, data_dir: &Path) -> Result<(), String> {
        let entries = crate::wpak::collect_dir(dir)?
            .into_iter()
            .map(|mut e| {
                e.compression = self.compression;
                // Attempt to load GUID from .meta sidecar.
                let asset_path = dir.join(&e.path);
                e.guid = crate::asset_id::read_meta(&asset_path).ok();
                e
            })
            .collect::<Vec<FileEntry>>();

        let resources = data_dir.join("resources");
        std::fs::create_dir_all(&resources)
            .map_err(|e| format!("Failed to create '{}': {}", resources.display(), e))?;
        let out_path = resources.join(out_name);
        crate::wpak::build_archive(&entries, out_path.to_str().unwrap())
    }

    // ------------------------------------------------------------------
    // Internal helpers
    // ------------------------------------------------------------------

    /// Find which archive (by index) contains a path. Returns `None` if not found.
    fn find_archive_source(&self, normalized: &str) -> Option<usize> {
        for (i, arc) in self.archives.iter().enumerate() {
            if arc.resolve_path(normalized).is_some() {
                return Some(i);
            }
        }
        None
    }
}

impl Default for AssetSystem {
    fn default() -> Self {
        Self::new()
    }
}
