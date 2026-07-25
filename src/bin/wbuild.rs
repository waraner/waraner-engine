use std::path::{Path, PathBuf};
use std::fs;

use serde::Deserialize;

use waraner_client as waraner_engine;

// --- .wproj format ----------------------------------------------------------

#[derive(Deserialize)]
struct Wproj {
    project: WprojProject,
    build: Option<WprojBuild>,
    runtime: Option<WprojRuntime>,
}

#[derive(Deserialize)]
struct WprojProject {
    name: String,
    version: Option<String>,
}

#[derive(Deserialize)]
struct WprojBuild {
    assets_dir: Option<String>,
    levels_dir: Option<String>,
    scripts_dir: Option<String>,
    output_dir: Option<String>,
}

#[derive(Deserialize)]
struct WprojRuntime {
    exe_path: Option<String>,
    dll_path: Option<String>,
}

// --- .wscene format ---------------------------------------------------------

#[derive(Deserialize)]
struct Wscene {
    settings: Option<WsceneSettings>,
    entities: Vec<WsceneEntity>,
}

#[derive(Deserialize)]
struct WsceneSettings {
    seed: Option<u64>,
}

#[derive(Deserialize)]
struct WsceneEntity {
    name: Option<String>,
    #[allow(dead_code)]
    entity_type: Option<String>,
    transform: Option<WsceneTransform>,
    color: Option<WsceneColor>,
    collider: Option<WsceneCollider>,
    script: Option<String>,
    model: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct WsceneTransform {
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 4]>,
    scale: Option<[f32; 3]>,
}

#[derive(Deserialize)]
struct WsceneColor {
    r: f32,
    g: f32,
    b: f32,
    a: Option<f32>,
}

#[derive(Deserialize)]
struct WsceneCollider {
    half_extents: [f32; 3],
}

fn find_wproj(dir: &Path) -> Option<PathBuf> {
    for entry in fs::read_dir(dir).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("wproj") {
            return Some(path);
        }
    }
    None
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage:");
        eprintln!("  wbuild new <project-name>   Create a new project scaffold");
        eprintln!("  wbuild <project-dir>        Build an existing project");
        std::process::exit(1);
    }

    if args[1] == "new" {
        if args.len() < 3 {
            eprintln!("error: missing project name\nUsage: wbuild new <project-name>");
            std::process::exit(1);
        }
        scaffold_project(&args[2]);
        return;
    }

    let project_dir = PathBuf::from(&args[1]);
    if !project_dir.is_dir() {
        eprintln!("error: '{}' is not a directory", project_dir.display());
        std::process::exit(1);
    }

    let project_name = project_dir
        .file_name()
        .unwrap_or_default()
        .to_str()
        .unwrap_or("game");

    // Find .wproj file
    let wproj_path = find_wproj(&project_dir).unwrap_or_else(|| {
        project_dir.join(format!("{}.wproj", project_name))
    });

    let wproj_content = match fs::read_to_string(&wproj_path) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("warning: no .wproj file found, using defaults");
            String::new()
        }
    };

    let wproj: Wproj = toml::from_str(&wproj_content).unwrap_or_else(|_| {
        Wproj {
            project: WprojProject {
                name: project_name.to_string(),
                version: None,
            },
            build: None,
            runtime: None,
        }
    });

    let project_name_cfg = wproj.project.name.clone();

    let build = wproj.build.unwrap_or(WprojBuild {
        assets_dir: None,
        levels_dir: None,
        scripts_dir: None,
        output_dir: None,
    });

    let runtime = wproj.runtime.unwrap_or(WprojRuntime {
        exe_path: None,
        dll_path: None,
    });

    let assets_dir = project_dir.join(build.assets_dir.as_deref().unwrap_or("assets"));
    let levels_dir = project_dir.join(build.levels_dir.as_deref().unwrap_or("levels"));
    let scripts_dir = project_dir.join(build.scripts_dir.as_deref().unwrap_or("scripts"));
    let output_dir = project_dir.join(build.output_dir.as_deref().unwrap_or("build"));

    let exe_path = runtime.exe_path.as_deref().unwrap_or("target/debug/game.exe");
    let dll_path = runtime.dll_path.as_deref().unwrap_or("target/debug/waraner_client.dll");

    println!("Building project: {}", wproj.project.name);
    println!("  Output: {}", output_dir.display());

    // Create output directories
    let resources_dir = output_dir.join("resources");
    let levels_out = output_dir.join("levels");
    let scripts_out = output_dir.join("scripts");
    let bin_out = output_dir.join("bin");

    for dir in [&resources_dir, &levels_out, &scripts_out, &bin_out] {
        fs::create_dir_all(dir).unwrap_or_else(|e| {
            eprintln!("error: failed to create directory '{}': {}", dir.display(), e);
            std::process::exit(1);
        });
    }

    // Step 1: Pack assets into WPAK archives
    let asset_archives = if assets_dir.is_dir() {
        pack_assets(&assets_dir, &resources_dir)
    } else {
        println!("  [skip] assets/ not found");
        Vec::new()
    };

    // Step 2: Compile Lua scripts
    if scripts_dir.is_dir() {
        compile_scripts(&scripts_dir, &scripts_out);
    } else {
        println!("  [skip] scripts/ not found");
    }

    // Step 3: Convert .wscene → .wmap
    if levels_dir.is_dir() {
        convert_levels(&levels_dir, &output_dir.join("levels"));
    } else {
        println!("  [skip] levels/ not found");
    }

    // Step 4: Copy runtime binaries
    copy_runtime(&exe_path, &dll_path, &output_dir, &project_name);

    // Step 5: Generate configs/game.cfg (build-time config)
    generate_game_config(&output_dir, &project_name_cfg, &asset_archives);

    println!("\nBuild complete: {}", output_dir.display());
}

// --- Project scaffolding ----------------------------------------------------

fn scaffold_project(name: &str) {
    let root = PathBuf::from(name);
    if root.exists() {
        eprintln!("error: '{}' already exists", root.display());
        std::process::exit(1);
    }

    let assets_dirs = ["models", "textures", "sounds", "shaders"];
    let dirs = [
        root.join("assets"),
        root.join("scripts"),
        root.join("levels"),
    ];

    for d in &dirs {
        fs::create_dir_all(d).unwrap_or_else(|e| {
            eprintln!("error: failed to create '{}': {}", d.display(), e);
            std::process::exit(1);
        });
    }

    for sub in &assets_dirs {
        fs::create_dir_all(root.join("assets").join(sub)).unwrap_or_else(|e| {
            eprintln!("error: failed to create assets/{}: {}", sub, e);
            std::process::exit(1);
        });
    }

    // .wproj
    let wproj_content = format!(
        r#"[project]
name = "{}"
version = "0.1.0"

[build]
assets_dir = "assets"
scripts_dir = "scripts"
levels_dir = "levels"
output_dir = "build"

[runtime]
exe_path = "target/debug/game.exe"
dll_path = "target/debug/waraner_client.dll"
"#,
        name
    );
    fs::write(root.join(format!("{}.wproj", name)), &wproj_content).unwrap_or_else(|e| {
        eprintln!("error: failed to write .wproj: {}", e);
        std::process::exit(1);
    });

    // main.lua
    let main_lua = r#"-- Waraner Engine entry point
function init()
    print("Hello from " .. engine.config:get("window_title"))
end

function update(dt)
end
"#;
    fs::write(root.join("scripts").join("main.lua"), main_lua).unwrap_or_else(|e| {
        eprintln!("error: failed to write scripts/main.lua: {}", e);
        std::process::exit(1);
    });

    let display_path = root.canonicalize().unwrap_or_else(|_| root.clone());
    println!("Created project '{}' in {}/", name, display_path.display());
    println!("  {}.wproj", name);
    println!("  assets/{{{}}}", assets_dirs.join(", "));
    println!("  scripts/main.lua");
    println!("  levels/");
    println!("\nRun 'wbuild {}' to build.", root.display());
}

// --- Asset packing ----------------------------------------------------------

fn pack_assets(assets_dir: &Path, resources_dir: &Path) -> Vec<String> {
    let subdirs = ["models", "textures", "sounds", "shaders"];
    let mut archives = Vec::new();

    for subdir in &subdirs {
        let dir = assets_dir.join(subdir);
        if !dir.is_dir() {
            continue;
        }

        let entries = match waraner_engine::wpak::collect_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                eprintln!("  [warn] failed to scan {}: {}", dir.display(), e);
                continue;
            }
        };

        if entries.is_empty() {
            continue;
        }

        let pak_name = format!("pak_{:04}.wpak", archives.len());
        let pak_path = resources_dir.join(&pak_name);
        match waraner_engine::wpak::build_archive(&entries, pak_path.to_str().unwrap()) {
            Ok(()) => {
                println!("  packed {} -> resources/{} ({} files)", subdir, pak_name, entries.len());
                archives.push(pak_name);
            }
            Err(e) => {
                eprintln!("  [error] failed to pack {}: {}", subdir, e);
            }
        }
    }

    archives
}

// --- Lua compilation --------------------------------------------------------

fn compile_scripts(scripts_dir: &Path, scripts_out: &Path) {
    let _lua = mlua::Lua::new();

    let entries = match fs::read_dir(scripts_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("  [error] failed to read scripts/: {}", e);
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            _ => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("lua") {
            continue;
        }

        let code = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  [error] failed to read '{}': {}", path.display(), e);
                continue;
            }
        };

        let bytecode = match lua_compile(&code) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("  [error] failed to compile '{}': {}", path.display(), e);
                continue;
            }
        };

        let stem = path.file_stem().unwrap().to_str().unwrap();
        let out_path = scripts_out.join(format!("{}.luac", stem));
        if let Err(e) = fs::write(&out_path, &bytecode) {
            eprintln!("  [error] failed to write '{}': {}", out_path.display(), e);
            continue;
        }
        println!("  compiled {} -> scripts/{}.luac", path.file_name().unwrap().to_str().unwrap(), stem);
    }
}

fn lua_compile(code: &str) -> Result<Vec<u8>, String> {
    let lua = mlua::Lua::new();
    let func = lua
        .load(code)
        .set_name("main")
        .into_function()
        .map_err(|e| format!("{}", e))?;
    Ok(func.dump(false))
}

// --- WSCENE → WMAP conversion ---------------------------------------------

fn convert_levels(levels_dir: &Path, levels_out: &Path) {
    let entries = match fs::read_dir(levels_dir) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("  [error] failed to read levels/: {}", e);
            return;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            _ => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("wscene") {
            continue;
        }

        let content = match fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("  [error] failed to read '{}': {}", path.display(), e);
                continue;
            }
        };

        let scene: Wscene = match toml::from_str(&content) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("  [error] failed to parse '{}': {}", path.display(), e);
                continue;
            }
        };

        let seed = scene.settings.as_ref().and_then(|s| s.seed).unwrap_or(0);
        let world = build_world(&scene);

        let stem = path.file_stem().unwrap().to_str().unwrap();
        let out_path = levels_out.join(format!("{}.wmap", stem));
        if let Err(e) = waraner_engine::wmap::write_world(&world, out_path.to_str().unwrap(), seed) {
            eprintln!("  [error] failed to write '{}': {}", out_path.display(), e);
            continue;
        }
        println!("  converted {} -> levels/{}.wmap", path.file_name().unwrap().to_str().unwrap(), stem);
    }
}

fn build_world(scene: &Wscene) -> waraner_engine::ecs::World {
    let mut world = waraner_engine::ecs::World::new();

    for entity_def in &scene.entities {
        let e = world.spawn();

        if let Some(name) = &entity_def.name {
            world.set_name(e, name);
        }

        // Apply entity type template first (if specified)
        if let Some(type_name) = &entity_def.entity_type {
            world.set_entity_type(e, type_name);
            waraner_engine::entity_types::apply_type(&mut world, e, type_name);
        }

        // Transform
        if let Some(t) = &entity_def.transform {
            let pos = t.position.unwrap_or([0.0, 0.0, 0.0]);
            let rot = t.rotation.unwrap_or([0.0, 0.0, 0.0, 1.0]);
            let scale = t.scale.unwrap_or([1.0, 1.0, 1.0]);
            world.add_transform(e, waraner_engine::ecs::Transform3D {
                position: glam::Vec3::new(pos[0], pos[1], pos[2]),
                rotation: glam::Quat::from_xyzw(rot[0], rot[1], rot[2], rot[3]),
                scale: glam::Vec3::new(scale[0], scale[1], scale[2]),
            });
        }

        // Color
        if let Some(c) = &entity_def.color {
            let a = c.a.unwrap_or(1.0);
            world.set_color(e, waraner_engine::ecs::Color { rgba: [c.r, c.g, c.b, a] });
        }

        // Collider
        if let Some(col) = &entity_def.collider {
            world.add_collider(e, waraner_engine::ecs::Collider {
                half_extents: glam::Vec3::new(col.half_extents[0], col.half_extents[1], col.half_extents[2]),
            });
        }

        // Model
        if let Some(model_path) = &entity_def.model {
            world.add_model(e, waraner_engine::ecs::Model { path: model_path.clone() });
        }

        // Script
        if let Some(script_name) = &entity_def.script {
            world.add_script(e, waraner_engine::ecs::ScriptComponent { script_name: script_name.clone() });
        }

        // Tags
        if let Some(tags) = &entity_def.tags {
            for tag in tags {
                match tag.as_str() {
                    "Player" => world.add_player(e),
                    "Ground" => world.add_ground(e),
                    "Static" => world.add_static(e),
                    "Sensor" => world.add_sensor(e),
                    "Camera" => world.add_camera(e, waraner_engine::ecs::Camera::default()),
                    "AudioListener" => world.add_audio_listener(e),
                    _ => {}
                }
            }
        }
    }

    world
}

// --- Config generation -------------------------------------------------------

fn generate_game_config(output_dir: &Path, project_name: &str, asset_archives: &[String]) {
    let configs_dir = output_dir.join("configs");
    fs::create_dir_all(&configs_dir).unwrap_or_else(|e| {
        eprintln!("error: failed to create configs dir: {}", e);
        std::process::exit(1);
    });

    let window_title = project_name.to_string();
    let main_script = "main".to_string();

    let cfg = format!(
        r#"window_title = {:?}
log_level = "info"
main_script = {:?}
asset_archives = {:?}
data_path = "."
"#,
        window_title, main_script, asset_archives,
    );

    let path = configs_dir.join("game.cfg");
    match fs::write(&path, &cfg) {
        Ok(()) => println!("  generated configs/game.cfg"),
        Err(e) => eprintln!("  [error] failed to write configs/game.cfg: {}", e),
    }
}

// --- Runtime copy ----------------------------------------------------------

fn copy_runtime(exe_path: &str, dll_path: &str, output_dir: &Path, project_name: &str) {
    let exe_dest = output_dir.join(format!("{}.exe", project_name));
    let dll_dest = output_dir.join("bin").join("waraner_client.dll");

    if let Err(e) = fs::copy(exe_path, &exe_dest) {
        eprintln!("  [error] failed to copy exe from '{}': {}", exe_path, e);
    } else {
        println!("  copied {} -> {}.exe", exe_path, project_name);
    }

    if let Err(e) = fs::copy(dll_path, &dll_dest) {
        eprintln!("  [error] failed to copy dll from '{}': {}", dll_path, e);
    } else {
        println!("  copied {} -> bin/waraner_client.dll", dll_path);
    }
}
