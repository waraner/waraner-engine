use std::path::Path;

pub struct MeshData {
    pub name: String,
    pub vertices: Vec<VertexData>,
    pub indices: Vec<u32>,
}

pub struct VertexData {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

pub fn load_model(path: &str) -> Result<Vec<MeshData>, String> {
    let ext = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    match ext.as_str() {
        "obj" => load_obj(path),
        "gltf" | "glb" => load_gltf(path),
        "wmesh" => load_wmesh(path),
        _ => Err(format!("Unsupported model format: {ext} (supported: obj, gltf, glb, wmesh)")),
    }
}

fn load_obj(path: &str) -> Result<Vec<MeshData>, String> {
    let load_opts = tobj::LoadOptions {
        triangulate: true,
        single_index: true,
        ignore_points: true,
        ignore_lines: true,
    };
    let (models, materials) = tobj::load_obj(path, &load_opts)
        .map_err(|e| format!("Failed to load OBJ '{path}': {e}"))?;
    let _ = materials;

    log::info!("OBJ '{}': {} model(s) loaded", path, models.len());

    let mut result = Vec::new();
    for (mi, model) in models.iter().enumerate() {
        let mesh = &model.mesh;
        let name = if model.name.is_empty() || model.name == "unnamed_object" {
            let stem = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("model");
            if models.len() > 1 {
                format!("{stem}_{mi}")
            } else {
                stem.to_string()
            }
        } else {
            model.name.clone()
        };

        let has_normals = !mesh.normals.is_empty();
        let has_uvs = !mesh.texcoords.is_empty();
        let vertex_count = mesh.positions.len() / 3;
        let index_count = mesh.indices.len();

        log::info!(
            "  [{}] name='{name}' vertices={vertex_count} indices={index_count} has_normals={has_normals} has_uvs={has_uvs}",
            mi
        );

        if vertex_count == 0 || index_count == 0 {
            log::warn!("  [{}] skipping empty mesh", mi);
            continue;
        }

        let mut vertices: Vec<VertexData> = Vec::with_capacity(vertex_count);
        for i in 0..vertex_count {
            let mut normal = [0.0f32; 3];
            if has_normals {
                let ni = i * 3;
                normal = [mesh.normals[ni], mesh.normals[ni + 1], mesh.normals[ni + 2]];
            }

            let mut uv = [0.0f32; 2];
            if has_uvs {
                let ui = i * 2;
                uv = [mesh.texcoords[ui], mesh.texcoords[ui + 1]];
            }

            let pi = i * 3;
            vertices.push(VertexData {
                position: [mesh.positions[pi], mesh.positions[pi + 1], mesh.positions[pi + 2]],
                normal,
                uv,
            });
        }

        let mut indices: Vec<u32> = Vec::with_capacity(mesh.indices.len());
        for &idx in &mesh.indices {
            indices.push(idx);
        }

        if !has_normals {
            compute_normals(&mut vertices, &indices);
        }

        result.push(MeshData { name, vertices, indices });
    }

    Ok(result)
}

fn load_gltf(path: &str) -> Result<Vec<MeshData>, String> {
    let (doc, buffers, _images) = gltf::import(path)
        .map_err(|e| format!("Failed to load glTF '{path}': {e}"))?;

    let mut result = Vec::new();
    for mesh in doc.meshes() {
        let mesh_name = mesh.name().unwrap_or("mesh").to_string();
        for (prim_idx, primitive) in mesh.primitives().enumerate() {
            let reader = primitive.reader(|buffer| Some(&buffers[buffer.index()]));

            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .map(|iter| iter.collect())
                .unwrap_or_default();

            let normals: Vec<[f32; 3]> = reader
                .read_normals()
                .map(|iter| iter.collect())
                .unwrap_or_default();

            let texcoords: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|data| data.into_f32().collect())
                .unwrap_or_default();

            let indices: Vec<u32> = reader
                .read_indices()
                .map(|iter| iter.into_u32().collect())
                .unwrap_or_default();

            let name = if doc.meshes().count() > 1 {
                format!("{mesh_name}_{prim_idx}")
            } else {
                mesh_name.clone()
            };

            let mut vertices: Vec<VertexData> = Vec::with_capacity(positions.len());
            for (i, &pos) in positions.iter().enumerate() {
                let normal = normals.get(i).copied().unwrap_or([0.0; 3]);
                let uv = texcoords.get(i).copied().unwrap_or([0.0; 2]);
                vertices.push(VertexData { position: pos, normal, uv });
            }

            let mut mesh_data = MeshData { name, vertices, indices };
            if normals.is_empty() {
                compute_normals(&mut mesh_data.vertices, &mesh_data.indices);
            }
            result.push(mesh_data);
        }
    }

    Ok(result)
}

fn load_wmesh(path: &str) -> Result<Vec<MeshData>, String> {
    let wmesh = crate::wmesh::read_wmesh(path)?;
    let name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("mesh");
    log::info!(
        "WMESH '{}': {} vertices, {} indices, {} submeshes",
        path,
        wmesh.vertices.len(),
        wmesh.indices.len(),
        wmesh.submeshes.len()
    );
    Ok(crate::wmesh::wmesh_to_meshes(&wmesh, name))
}

fn compute_normals(vertices: &mut [VertexData], indices: &[u32]) {
    for v in vertices.iter_mut() {
        v.normal = [0.0; 3];
    }
    for tri in indices.chunks(3) {
        if tri.len() < 3 {
            continue;
        }
        let i0 = tri[0] as usize;
        let i1 = tri[1] as usize;
        let i2 = tri[2] as usize;
        if i0 >= vertices.len() || i1 >= vertices.len() || i2 >= vertices.len() {
            continue;
        }
        let a = vertices[i0].position;
        let b = vertices[i1].position;
        let c = vertices[i2].position;
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len > 0.0 {
            for i in &[i0, i1, i2] {
                vertices[*i].normal[0] += n[0] / len;
                vertices[*i].normal[1] += n[1] / len;
                vertices[*i].normal[2] += n[2] / len;
            }
        }
    }
    for v in vertices.iter_mut() {
        let len = (v.normal[0] * v.normal[0]
            + v.normal[1] * v.normal[1]
            + v.normal[2] * v.normal[2])
        .sqrt();
        if len > 0.0 {
            v.normal[0] /= len;
            v.normal[1] /= len;
            v.normal[2] /= len;
        }
    }
}


