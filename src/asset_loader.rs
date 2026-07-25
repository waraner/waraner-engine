use std::sync::Arc;
use std::thread;

use crossbeam::channel;

use crate::asset_id::AssetId;
use crate::asset_system::AssetSystem;

// ---------------------------------------------------------------------------
// Commands sent from main thread → asset loader thread
// ---------------------------------------------------------------------------

enum AssetCommand {
    Load(String),
    LoadByGuid(uuid::Uuid),
    Shutdown,
}

// ---------------------------------------------------------------------------
// Results sent from asset loader thread → main thread
// ---------------------------------------------------------------------------

pub struct AssetLoadResult {
    pub id: AssetId,
    pub bytes: Result<Arc<Vec<u8>>, String>,
}

// ---------------------------------------------------------------------------
// Asset loader thread handle (main thread side)
// ---------------------------------------------------------------------------

pub struct AssetLoader {
    cmd_tx: channel::Sender<AssetCommand>,
    result_rx: channel::Receiver<AssetLoadResult>,
    handle: Option<thread::JoinHandle<()>>,
}

impl AssetLoader {
    /// Spawn an asset loader thread that owns the given `AssetSystem`.
    /// The system is moved to the thread and is not accessible from main.
    pub fn new(system: AssetSystem) -> Self {
        let (cmd_tx, cmd_rx) = channel::unbounded::<AssetCommand>();
        let (result_tx, result_rx) = channel::unbounded::<AssetLoadResult>();

        let handle = thread::Builder::new()
            .name("asset-loader".into())
            .spawn(move || {
                let mut sys = system;
                for cmd in &cmd_rx {
                    match cmd {
                        AssetCommand::Load(path) => {
                            let bytes = sys.get_bytes(&path);
                            let id = if bytes.is_ok() {
                                let guid = sys
                                    .all_guid_pairs()
                                    .get(&crate::wpak::fnv1a(&path))
                                    .copied()
                                    .unwrap_or_else(uuid::Uuid::new_v4);
                                AssetId::for_path_with_guid(&path, guid)
                            } else {
                                AssetId::for_path(&path)
                            };
                            let _ = result_tx.send(AssetLoadResult { id, bytes });
                        }
                        AssetCommand::LoadByGuid(guid) => {
                            let bytes = sys.get_bytes_by_guid(&guid);
                            let id = match &bytes {
                                Ok(_) => {
                                    let path = sys
                                        .resolve_guid_path(&guid)
                                        .unwrap_or_else(|| guid.to_string());
                                    AssetId::for_path_with_guid(&path, guid)
                                }
                                Err(_) => AssetId {
                                    guid,
                                    path_hash: 0,
                                    path: guid.to_string(),
                                },
                            };
                            let _ = result_tx.send(AssetLoadResult { id, bytes });
                        }
                        AssetCommand::Shutdown => break,
                    }
                }
            })
            .expect("failed to spawn asset loader thread");

        Self {
            cmd_tx,
            result_rx,
            handle: Some(handle),
        }
    }

    /// Enqueue an asset load request by path.
    pub fn request_load(&self, path: &str) {
        let _ = self.cmd_tx.send(AssetCommand::Load(path.to_string()));
    }

    /// Enqueue an asset load request by GUID.
    pub fn request_load_by_guid(&self, guid: uuid::Uuid) {
        let _ = self.cmd_tx.send(AssetCommand::LoadByGuid(guid));
    }

    /// Drain all results that have arrived from the asset thread.
    pub fn drain_results(&mut self) -> Vec<AssetLoadResult> {
        let mut results = Vec::new();
        while let Ok(r) = self.result_rx.try_recv() {
            results.push(r);
        }
        results
    }

    pub fn shutdown(&mut self) {
        let _ = self.cmd_tx.send(AssetCommand::Shutdown);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for AssetLoader {
    fn drop(&mut self) {
        self.shutdown();
    }
}
