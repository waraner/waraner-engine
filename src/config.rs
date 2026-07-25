use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// TOML config file structs
// ---------------------------------------------------------------------------

/// Game.cfg — generated at build time, read-only at runtime.
#[derive(Debug, Deserialize, Serialize)]
struct GameConfig {
    window_title: Option<String>,
    log_level: Option<String>,
    main_script: Option<String>,
    asset_archives: Option<Vec<String>>,
    data_path: Option<String>,
}

/// User.cfg — user-modifiable settings, saved via Lua API.
#[derive(Debug, Deserialize, Serialize, Clone)]
struct UserConfig {
    window_width: Option<u32>,
    window_height: Option<u32>,
    fullscreen: Option<bool>,
    vsync: Option<bool>,
}

// ---------------------------------------------------------------------------
// WaranerConfig
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct WaranerConfig {
    pub window_title: String,
    pub window_width: u32,
    pub window_height: u32,
    pub fullscreen: bool,
    pub vsync: bool,
    pub data_path: PathBuf,
    pub asset_archives: Vec<String>,
    pub main_script: String,
    pub log_level: String,
    /// Directory where config files are stored (data_path / configs).
    pub config_dir: PathBuf,
}

impl Default for WaranerConfig {
    fn default() -> Self {
        Self {
            window_title: "Waraner Engine".to_string(),
            window_width: 1280,
            window_height: 720,
            fullscreen: false,
            vsync: true,
            data_path: PathBuf::from("."),
            asset_archives: vec![
                "pak_0000.wpak".to_string(),
                "pak_0001.wpak".to_string(),
                "pak_0002.wpak".to_string(),
                "pak_0003.wpak".to_string(),
            ],
            main_script: "main".to_string(),
            log_level: "info".to_string(),
            config_dir: PathBuf::from("configs"),
        }
    }
}

impl WaranerConfig {
    /// Build config from defaults, then overlay config files, then env/args.
    pub fn from_env_and_args() -> Self {
        let mut config = Self::default();

        // 1. WARANER_DATA_DIR env var sets data_path before files are loaded
        if let Ok(dir) = std::env::var("WARANER_DATA_DIR") {
            config.data_path = PathBuf::from(dir);
        }

        // 2. Load config files
        config.load_from_files();

        // 3. CLI args override everything
        let args: Vec<String> = std::env::args().collect();
        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--data-path" | "--data_dir" => {
                    if i + 1 < args.len() {
                        config.data_path = PathBuf::from(&args[i + 1]);
                        i += 1;
                    }
                }
                "--width" => {
                    if i + 1 < args.len() {
                        config.window_width = args[i + 1].parse().unwrap_or(1280);
                        i += 1;
                    }
                }
                "--height" => {
                    if i + 1 < args.len() {
                        config.window_height = args[i + 1].parse().unwrap_or(720);
                        i += 1;
                    }
                }
                "--title" => {
                    if i + 1 < args.len() {
                        config.window_title = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--fullscreen" => config.fullscreen = true,
                "--vsync" => config.vsync = true,
                "--no-vsync" => config.vsync = false,
                "--log-level" => {
                    if i + 1 < args.len() {
                        config.log_level = args[i + 1].clone();
                        i += 1;
                    }
                }
                "--main-script" => {
                    if i + 1 < args.len() {
                        config.main_script = args[i + 1].clone();
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }

        config
    }

    /// Load `configs/game.cfg` and `configs/user.cfg` from the data directory.
    /// game.cfg values are overlaid first, then user.cfg values.
    fn load_from_files(&mut self) {
        self.config_dir = self.data_path.join("configs");

        // game.cfg — build-time settings
        let game_path = self.config_dir.join("game.cfg");
        if let Ok(content) = std::fs::read_to_string(&game_path) {
            if let Ok(cfg) = toml::from_str::<GameConfig>(&content) {
                if let Some(v) = cfg.window_title { self.window_title = v; }
                if let Some(v) = cfg.log_level { self.log_level = v; }
                if let Some(v) = cfg.main_script { self.main_script = v; }
                if let Some(v) = cfg.asset_archives { self.asset_archives = v; }
                if let Some(v) = cfg.data_path { self.data_path = PathBuf::from(v); }
                log::info!("Loaded config from {}", game_path.display());
            } else {
                log::warn!("Failed to parse {}", game_path.display());
            }
        } else {
            log::debug!("{} not found, using defaults", game_path.display());
        }

        // user.cfg — user preferences
        let user_path = self.config_dir.join("user.cfg");
        if let Ok(content) = std::fs::read_to_string(&user_path) {
            if let Ok(cfg) = toml::from_str::<UserConfig>(&content) {
                if let Some(v) = cfg.window_width { self.window_width = v; }
                if let Some(v) = cfg.window_height { self.window_height = v; }
                if let Some(v) = cfg.fullscreen { self.fullscreen = v; }
                if let Some(v) = cfg.vsync { self.vsync = v; }
                log::info!("Loaded config from {}", user_path.display());
            } else {
                log::warn!("Failed to parse {}", user_path.display());
            }
        } else {
            log::debug!("{} not found, using defaults", user_path.display());
        }
    }

    /// Save current user-configurable settings to `configs/user.cfg`.
    /// Returns the path that was written to.
    pub fn save_user_config(&self) -> Result<PathBuf, String> {
        let cfg = UserConfig {
            window_width: Some(self.window_width),
            window_height: Some(self.window_height),
            fullscreen: Some(self.fullscreen),
            vsync: Some(self.vsync),
        };
        let toml_str =
            toml::to_string_pretty(&cfg).map_err(|e| format!("config serialize: {e}"))?;
        let path = self.config_dir.join("user.cfg");
        std::fs::create_dir_all(&self.config_dir)
            .map_err(|e| format!("cannot create config dir: {e}"))?;
        std::fs::write(&path, &toml_str).map_err(|e| format!("cannot write user.cfg: {e}"))?;
        log::info!("Saved user config to {}", path.display());
        Ok(path)
    }
}
