use std::fs;
use std::io::{Read, Write};

use bytemuck::{cast_slice, Pod, Zeroable};

use crate::model_loader::{MeshData, VertexData};

const WMESH_MAGIC: [u8; 8] = *b"WMESH\0\0\0";
const WMESH_VERSION: u32 = 1;

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct Header {
    magic: [u8; 8],
    version: u32,
    vertex_count: u32,
    index_count: u32,
    mesh_count: u32,
    material_count: u32,
    has_bones: u32,
    _reserved: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct VertexRaw {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
    tangent: [f32; 4],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct SubmeshRaw {
    index_offset: u32,
    index_count: u32,
    material_id: u32,
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BoneMatrixRaw {
    data: [f32; 16],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct BoneWeightRaw {
    weights: [f32; 4],
    indices: [u32; 4],
}

#[allow(dead_code)]
pub struct WmeshVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
    pub tangent: [f32; 4],
    pub color: [f32; 4],
}

#[allow(dead_code)]
pub struct WmeshSubmesh {
    pub index_offset: u32,
    pub index_count: u32,
    pub material_id: u32,
}

#[allow(dead_code)]
pub struct Wmesh {
    pub vertices: Vec<WmeshVertex>,
    pub indices: Vec<u32>,
    pub submeshes: Vec<WmeshSubmesh>,
    pub bone_matrices: Vec<[[f32; 4]; 4]>,
    pub bone_weights: Vec<[f32; 4]>,
    pub bone_indices: Vec<[u32; 4]>,
}

pub fn read_wmesh(path: &str) -> Result<Wmesh, String> {
    let mut file = fs::File::open(path).map_err(|e| format!("Failed to open '{path}': {e}"))?;

    let mut header_bytes = [0u8; std::mem::size_of::<Header>()];
    file.read_exact(&mut header_bytes)
        .map_err(|e| format!("Failed to read header: {e}"))?;
    let header: &Header = try_pod_ref(&header_bytes)?;

    if header.magic != WMESH_MAGIC {
        return Err(format!("Invalid magic: expected WMESH, got {:?}", std::str::from_utf8(&header.magic)));
    }
    if header.version != WMESH_VERSION {
        return Err(format!("Unsupported version: {} (expected {})", header.version, WMESH_VERSION));
    }

    let vertex_count = header.vertex_count as usize;
    let index_count = header.index_count as usize;
    let mesh_count = header.mesh_count as usize;
    let has_bones = header.has_bones != 0;

    let mut vertices = Vec::with_capacity(vertex_count);
    let mut vert_buf = vec![0u8; vertex_count * std::mem::size_of::<VertexRaw>()];
    file.read_exact(&mut vert_buf)
        .map_err(|e| format!("Failed to read vertices: {e}"))?;
    let verts_raw: &[VertexRaw] = cast_slice(&vert_buf);
    for v in verts_raw {
        vertices.push(WmeshVertex {
            position: v.position,
            normal: v.normal,
            uv: v.uv,
            tangent: v.tangent,
            color: v.color,
        });
    }

    let mut indices = Vec::with_capacity(index_count);
    let mut idx_buf = vec![0u8; index_count * 4];
    file.read_exact(&mut idx_buf)
        .map_err(|e| format!("Failed to read indices: {e}"))?;
    indices.extend_from_slice(cast_slice::<u8, u32>(&idx_buf));

    let mut submeshes = Vec::with_capacity(mesh_count);
    let mut sub_buf = vec![0u8; mesh_count * std::mem::size_of::<SubmeshRaw>()];
    if !sub_buf.is_empty() {
        file.read_exact(&mut sub_buf)
            .map_err(|e| format!("Failed to read submeshes: {e}"))?;
        let subs_raw: &[SubmeshRaw] = cast_slice(&sub_buf);
        for s in subs_raw {
            submeshes.push(WmeshSubmesh {
                index_offset: s.index_offset,
                index_count: s.index_count,
                material_id: s.material_id,
            });
        }
    }

    let mut bone_matrices = Vec::new();
    let mut bone_weights = Vec::new();
    let mut bone_indices = Vec::new();
    if has_bones {
        let mat_count = (vertex_count + 3) / 4;
        let mut mat_buf = vec![0u8; mat_count * std::mem::size_of::<BoneMatrixRaw>()];
        if !mat_buf.is_empty() {
            file.read_exact(&mut mat_buf)
                .map_err(|e| format!("Failed to read bone matrices: {e}"))?;
            let mats_raw: &[BoneMatrixRaw] = cast_slice(&mat_buf);
            for m in mats_raw {
                bone_matrices.push([
                    [m.data[0], m.data[1], m.data[2], m.data[3]],
                    [m.data[4], m.data[5], m.data[6], m.data[7]],
                    [m.data[8], m.data[9], m.data[10], m.data[11]],
                    [m.data[12], m.data[13], m.data[14], m.data[15]],
                ]);
            }
        }

        let mut bw_buf = vec![0u8; vertex_count * std::mem::size_of::<BoneWeightRaw>()];
        if !bw_buf.is_empty() {
            file.read_exact(&mut bw_buf)
                .map_err(|e| format!("Failed to read bone weights: {e}"))?;
            let bw_raw: &[BoneWeightRaw] = cast_slice(&bw_buf);
            for bw in bw_raw {
                bone_weights.push(bw.weights);
                bone_indices.push(bw.indices);
            }
        }
    }

    Ok(Wmesh { vertices, indices, submeshes, bone_matrices, bone_weights, bone_indices })
}

pub fn write_wmesh(path: &str, meshes: &[MeshData]) -> Result<(), String> {
    if meshes.is_empty() {
        return Err("No meshes to write".to_string());
    }

    let total_vertices: usize = meshes.iter().map(|m| m.vertices.len()).sum();
    let total_indices: usize = meshes.iter().map(|m| m.indices.len()).sum();

    let header = Header {
        magic: WMESH_MAGIC,
        version: WMESH_VERSION,
        vertex_count: total_vertices as u32,
        index_count: total_indices as u32,
        mesh_count: meshes.len() as u32,
        material_count: 0,
        has_bones: 0,
        _reserved: 0,
    };

    let mut verts_raw: Vec<VertexRaw> = Vec::with_capacity(total_vertices);
    let mut indices_raw: Vec<u32> = Vec::with_capacity(total_indices);
    let mut submeshes_raw: Vec<SubmeshRaw> = Vec::with_capacity(meshes.len());
    let mut vert_offset: u32 = 0;

    for md in meshes {
        for v in &md.vertices {
            verts_raw.push(VertexRaw {
                position: v.position,
                normal: v.normal,
                uv: v.uv,
                tangent: [0.0; 4],
                color: [1.0; 4],
            });
        }

        let idx_start = indices_raw.len() as u32;
        for &i in &md.indices {
            indices_raw.push(i + vert_offset);
        }
        submeshes_raw.push(SubmeshRaw {
            index_offset: idx_start,
            index_count: md.indices.len() as u32,
            material_id: 0,
        });

        vert_offset += md.vertices.len() as u32;
    }

    let mut file = fs::File::create(path)
        .map_err(|e| format!("Failed to create '{path}': {e}"))?;

    let header_bytes: &[u8] = cast_slice(std::slice::from_ref(&header));
    file.write_all(header_bytes).map_err(|e| format!("Failed to write header: {e}"))?;

    let verts_bytes: &[u8] = cast_slice(&verts_raw);
    file.write_all(verts_bytes).map_err(|e| format!("Failed to write vertices: {e}"))?;

    let idx_bytes: &[u8] = cast_slice(&indices_raw);
    file.write_all(idx_bytes).map_err(|e| format!("Failed to write indices: {e}"))?;

    if !submeshes_raw.is_empty() {
        let sub_bytes: &[u8] = cast_slice(&submeshes_raw);
        file.write_all(sub_bytes).map_err(|e| format!("Failed to write submeshes: {e}"))?;
    }

    Ok(())
}

pub fn wmesh_to_meshes(wmesh: &Wmesh, name: &str) -> Vec<MeshData> {
    let mut result = Vec::new();
    for (i, sub) in wmesh.submeshes.iter().enumerate() {
        let sub_name = if wmesh.submeshes.len() > 1 {
            format!("{name}_{i}")
        } else {
            name.to_string()
        };

        let start = sub.index_offset as usize;
        let end = start + sub.index_count as usize;
        let sub_indices: Vec<u32> = wmesh.indices[start..end]
            .iter()
            .map(|idx| *idx - sub.index_offset)
            .collect();

        let mut used: Vec<bool> = vec![false; wmesh.vertices.len()];
        for &idx in &sub_indices {
            if (idx as usize) < wmesh.vertices.len() {
                used[idx as usize] = true;
            }
        }

        let mut remap: Vec<u32> = vec![0u32; wmesh.vertices.len()];
        let mut new_vertices: Vec<VertexData> = Vec::new();
        for (vi, used) in used.iter().enumerate() {
            if *used {
                remap[vi] = new_vertices.len() as u32;
                let v = &wmesh.vertices[vi];
                new_vertices.push(VertexData {
                    position: v.position,
                    normal: v.normal,
                    uv: v.uv,
                });
            }
        }

        let new_indices: Vec<u32> = sub_indices.iter().map(|&idx| remap[idx as usize]).collect();

        result.push(MeshData {
            name: sub_name,
            vertices: new_vertices,
            indices: new_indices,
        });
    }
    result
}

fn try_pod_ref<T: Pod>(bytes: &[u8]) -> Result<&T, String> {
    let size = std::mem::size_of::<T>();
    if bytes.len() < size {
        return Err(format!("Buffer too small: {} < {}", bytes.len(), size));
    }
    bytemuck::try_from_bytes(&bytes[..size]).map_err(|e| format!("Pod cast failed: {e}"))
}
