use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::model_loader;
use crate::render_frame::*;

struct SendableImGuiContext(imgui::Context);
unsafe impl Send for SendableImGuiContext {}

// ============================================================================
// Vertex format
// ============================================================================

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
pub struct Vertex {
    position: [f32; 3],
    normal: [f32; 3],
    uv: [f32; 2],
}

const VERTEX_ATTRIBUTES: [wgpu::VertexAttribute; 3] = [
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
    wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 2 },
];

impl Vertex {
    fn desc() -> wgpu::VertexBufferLayout<'static> {
        wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as wgpu::BufferAddress,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &VERTEX_ATTRIBUTES,
        }
    }
}

// ============================================================================
// Built-in mesh data
// ============================================================================

const CUBE_VERTICES: &[Vertex] = &[
    Vertex { position: [-0.5, -0.5, 0.5], normal: [0., 0., 1.], uv: [0., 1.] },
    Vertex { position: [0.5, -0.5, 0.5], normal: [0., 0., 1.], uv: [1., 1.] },
    Vertex { position: [0.5, 0.5, 0.5], normal: [0., 0., 1.], uv: [1., 0.] },
    Vertex { position: [-0.5, 0.5, 0.5], normal: [0., 0., 1.], uv: [0., 0.] },
    Vertex { position: [-0.5, 0.5, -0.5], normal: [0., 0., -1.], uv: [0., 0.] },
    Vertex { position: [0.5, 0.5, -0.5], normal: [0., 0., -1.], uv: [1., 0.] },
    Vertex { position: [0.5, -0.5, -0.5], normal: [0., 0., -1.], uv: [1., 1.] },
    Vertex { position: [-0.5, -0.5, -0.5], normal: [0., 0., -1.], uv: [0., 1.] },
    Vertex { position: [-0.5, 0.5, 0.5], normal: [0., 1., 0.], uv: [0., 1.] },
    Vertex { position: [0.5, 0.5, 0.5], normal: [0., 1., 0.], uv: [1., 1.] },
    Vertex { position: [0.5, 0.5, -0.5], normal: [0., 1., 0.], uv: [1., 0.] },
    Vertex { position: [-0.5, 0.5, -0.5], normal: [0., 1., 0.], uv: [0., 0.] },
    Vertex { position: [-0.5, -0.5, -0.5], normal: [0., -1., 0.], uv: [0., 0.] },
    Vertex { position: [0.5, -0.5, -0.5], normal: [0., -1., 0.], uv: [1., 0.] },
    Vertex { position: [0.5, -0.5, 0.5], normal: [0., -1., 0.], uv: [1., 1.] },
    Vertex { position: [-0.5, -0.5, 0.5], normal: [0., -1., 0.], uv: [0., 1.] },
    Vertex { position: [0.5, -0.5, 0.5], normal: [1., 0., 0.], uv: [0., 1.] },
    Vertex { position: [0.5, -0.5, -0.5], normal: [1., 0., 0.], uv: [1., 1.] },
    Vertex { position: [0.5, 0.5, -0.5], normal: [1., 0., 0.], uv: [1., 0.] },
    Vertex { position: [0.5, 0.5, 0.5], normal: [1., 0., 0.], uv: [0., 0.] },
    Vertex { position: [-0.5, 0.5, -0.5], normal: [-1., 0., 0.], uv: [0., 0.] },
    Vertex { position: [-0.5, -0.5, -0.5], normal: [-1., 0., 0.], uv: [1., 0.] },
    Vertex { position: [-0.5, -0.5, 0.5], normal: [-1., 0., 0.], uv: [1., 1.] },
    Vertex { position: [-0.5, 0.5, 0.5], normal: [-1., 0., 0.], uv: [0., 1.] },
];

const CUBE_INDICES: &[u32] = &[
    0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7, 8, 9, 10, 8, 10, 11,
    12, 13, 14, 12, 14, 15, 16, 17, 18, 16, 18, 19, 20, 21, 22, 20, 22, 23,
];

const GROUND_VERTICES: &[Vertex] = &[
    Vertex { position: [-0.5, 0.5, -0.5], normal: [0.0, 1.0, 0.0], uv: [0., 1.] },
    Vertex { position: [0.5, 0.5, -0.5], normal: [0.0, 1.0, 0.0], uv: [1., 1.] },
    Vertex { position: [0.5, 0.5, 0.5], normal: [0.0, 1.0, 0.0], uv: [1., 0.] },
    Vertex { position: [-0.5, 0.5, 0.5], normal: [0.0, 1.0, 0.0], uv: [0., 0.] },
];

const GROUND_INDICES: &[u32] = &[0, 2, 1, 0, 3, 2];

// ============================================================================
// Texture formats
// ============================================================================

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const GB_ALBEDO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const GB_NORMAL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const GB_POSITION_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const SHADOW_MAP_SIZE: u32 = 2048;

// ============================================================================
// Uniform structs (must match WGSL layout)
// ============================================================================

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct GeometryUniforms {
    model: [[f32; 4]; 4],
    view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct LightUniforms {
    light_dir: [f32; 4],
    light_color: [f32; 4],
    ambient: [f32; 4],
    camera_pos: [f32; 4],
    light_view_proj: [[f32; 4]; 4],
    inv_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct TonemapUniforms {
    exposure: f32,
    bloom_intensity: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Copy, Clone, Pod, Zeroable)]
struct SkyUniforms {
    sky_color: [f32; 4],
    sky_brightness: f32,
    sky_intensity: f32,
    screen_w: f32,
    screen_h: f32,
    _pad: [f32; 4],
}

// ============================================================================
// WGSL shaders
// ============================================================================

const GEOMETRY_SHADER: &str = r#"
struct Uniforms {
    model: mat4x4<f32>,
    view_proj: mat4x4<f32>,
};
@binding(0) @group(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) world_pos: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
@vertex
fn vs_main(input: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    let world_pos = uniforms.model * vec4<f32>(input.position, 1.0);
    out.clip_position = uniforms.view_proj * world_pos;
    out.world_normal = normalize((uniforms.model * vec4<f32>(input.normal, 0.0)).xyz);
    out.world_pos = world_pos.xyz;
    out.uv = input.uv;
    return out;
}

@binding(0) @group(1) var material_texture: texture_2d<f32>;
@binding(1) @group(1) var material_sampler: sampler;

struct FragmentOutput {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) position: vec4<f32>,
};
@fragment
fn fs_main(input: VertexOutput) -> FragmentOutput {
    let tex = textureSample(material_texture, material_sampler, input.uv);
    var out: FragmentOutput;
    out.albedo = tex;
    out.normal = vec4<f32>(normalize(input.world_normal), 0.0);
    out.position = vec4<f32>(input.world_pos, 1.0);
    return out;
}
"#;

const SHADOW_SHADER: &str = r#"
struct Uniforms {
    model: mat4x4<f32>,
    light_vp: mat4x4<f32>,
};
@binding(0) @group(0) var<uniform> uniforms: Uniforms;

struct VertexInput {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
@vertex
fn vs_main(input: VertexInput) -> @builtin(position) vec4<f32> {
    return uniforms.light_vp * uniforms.model * vec4<f32>(input.position, 1.0);
}
"#;

const FULLSCREEN_QUAD_VS: &str = r#"
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4<f32> {
    let pos = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    return vec4<f32>(pos[idx], 0.0, 1.0);
}
"#;

const LIGHTING_SHADER: &str = r#"
@group(0) @binding(0) var albedo_tex: texture_2d<f32>;
@group(0) @binding(1) var normal_tex: texture_2d<f32>;
@group(0) @binding(2) var position_tex: texture_2d<f32>;
@group(0) @binding(3) var shadow_tex: texture_depth_2d;
@group(0) @binding(4) var shadow_sampler: sampler_comparison;
@group(0) @binding(5) var gb_depth: texture_depth_2d;

struct LightUniforms {
    light_dir: vec4<f32>,
    light_color: vec4<f32>,
    ambient: vec4<f32>,
    camera_pos: vec4<f32>,
    light_view_proj: mat4x4<f32>,
    inv_proj: mat4x4<f32>,
};
@group(1) @binding(0) var<uniform> light: LightUniforms;

struct SkyParams {
    sky_color: vec4<f32>,
    sky_brightness: f32,
    sky_intensity: f32,
    screen_w: f32,
    screen_h: f32,
};

@group(2) @binding(0) var<uniform> sky: SkyParams;

@fragment
fn fs_main(@builtin(position) coord: vec4<f32>) -> @location(0) vec4<f32> {
    let px = vec2<i32>(coord.xy);
    let albedo = textureLoad(albedo_tex, px, 0);
    let n_raw = textureLoad(normal_tex, px, 0).xyz;

    // Detect sky pixels via zero normal (G-buffer cleared to 0 where no geometry).
    if (length(n_raw) < 0.001) {
        // Sky background (flat color).
        let sky_bg = sky.sky_color.xyz * sky.sky_intensity * sky.sky_brightness;

        // Reconstruct world-space ray direction for this pixel.
        // view_proj uses OpenGL convention (Y-up NDC), but screen coords are Y-down,
        // so we flip Y to get the correct NDC before multiplying by inv_proj.
        let uv = vec2<f32>(
            (f32(px.x) + 0.5) / sky.screen_w,
            (f32(px.y) + 0.5) / sky.screen_h,
        );
        let ndc_ray = vec4<f32>(
            uv.x * 2.0 - 1.0,
            -(uv.y * 2.0 - 1.0),
            1.0,
            1.0,
        );
        let far_pos = light.inv_proj * ndc_ray;
        let far_ws = far_pos.xyz / far_pos.w;
        let ws_ray = normalize(far_ws - light.camera_pos.xyz);

        // Sun glow — Source-style: sharp disc + narrow glow.
        let L = normalize(light.light_dir.xyz);
        let sun_cos = max(dot(ws_ray, L), 0.0);

        // Sharp disc: narrow smoothstep (~0.25° half-angle).
        let disc = smoothstep(0.999, 0.9998, sun_cos) * 3.0;
        // Narrow glow: high exponent, only kicks in when close to sun.
        let glow = pow(sun_cos, 32.0) * 0.2;
        let sun = disc + glow;

        // HDR sun color (> 1.0) — tonemapper compresses it.
        let sun_color = light.light_color.xyz * 8.0;
        return vec4<f32>(sky_bg + sun * sun_color, 1.0);
    }

    let n = normalize(n_raw);
    let world_pos = textureLoad(position_tex, px, 0).xyz;

    let L = normalize(light.light_dir.xyz);
    let NdotL = max(dot(n, L), 0.0);

    // Shadow map sampling — 16-tap Poisson PCF.
    let light_clip = light.light_view_proj * vec4<f32>(world_pos, 1.0);
    let ndc_shadow = light_clip.xyz / light_clip.w;
    let shadow_uv = ndc_shadow.xy * vec2<f32>(0.5, -0.5) + 0.5;
    let bias = 0.005;
    let depth = ndc_shadow.z - bias;
    let spread = 2.0 / 2048.0;
    var shadow = 0.0;
    let poisson = array<vec2<f32>, 16>(
        vec2<f32>(-0.94201624, -0.39906216),
        vec2<f32>( 0.94558609, -0.76890725),
        vec2<f32>(-0.09418410, -0.92938870),
        vec2<f32>( 0.34495938,  0.29387760),
        vec2<f32>(-0.91588581,  0.45771432),
        vec2<f32>(-0.81544232, -0.87912464),
        vec2<f32>(-0.38277543,  0.27676845),
        vec2<f32>( 0.97484398,  0.75648379),
        vec2<f32>( 0.44323325, -0.97511554),
        vec2<f32>( 0.53742981, -0.47373420),
        vec2<f32>(-0.26496911, -0.41893023),
        vec2<f32>( 0.79197514,  0.19090188),
        vec2<f32>(-0.24188840,  0.99706507),
        vec2<f32>(-0.81409955,  0.91437590),
        vec2<f32>( 0.19984126,  0.78641367),
        vec2<f32>( 0.14383161, -0.14100790),
    );
    for (var i = 0u; i < 16u; i++) {
        let offset = poisson[i] * spread;
        shadow += textureSampleCompare(shadow_tex, shadow_sampler, shadow_uv + offset, depth);
    }
    shadow = shadow / 16.0;

    let diffuse = light.light_color.xyz * NdotL * shadow;
    let ambient_contrib = light.ambient.xyz * albedo.xyz;

    return vec4<f32>(albedo.xyz * diffuse + ambient_contrib, 1.0);
}
"#;

const TONEMAP_SHADER: &str = r#"
@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;
@group(0) @binding(2) var bloom_tex: texture_2d<f32>;
@group(0) @binding(3) var bloom_sampler: sampler;

struct TonemapParams {
    exposure: f32,
    bloom_intensity: f32,
    _pad: vec2<f32>,
};
@group(1) @binding(0) var<uniform> params: TonemapParams;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    let pos = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VertexOutput;
    out.clip = vec4<f32>(pos[idx], 0.0, 1.0);
    let uv = pos[idx] * 0.5 + 0.5;
    out.uv = vec2(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let hdr = textureSampleLevel(hdr_tex, hdr_sampler, input.uv, 0.0).rgb;
    let bloom = textureSampleLevel(bloom_tex, bloom_sampler, input.uv, 0.0).rgb;
    let combined = hdr + bloom * params.bloom_intensity;
    let exposed = combined * params.exposure;
    let mapped = exposed / (exposed + vec3<f32>(1.0));
    return vec4<f32>(pow(mapped, vec3<f32>(1.0 / 2.2)), 1.0);
}
"#;

const BLOOM_EXTRACT_SHADER: &str = r#"
@group(0) @binding(0) var hdr_tex: texture_2d<f32>;
@group(0) @binding(1) var hdr_sampler: sampler;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    let pos = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VertexOutput;
    out.clip = vec4<f32>(pos[idx], 0.0, 1.0);
    let uv = pos[idx] * 0.5 + 0.5;
    out.uv = vec2(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(hdr_tex, hdr_sampler, input.uv, 0.0).rgb;
    let brightness = max(c.r, max(c.g, c.b));
    let threshold = 1.0;
    let soft = clamp((brightness - threshold + 0.5) / 0.5, 0.0, 1.0);
    return vec4<f32>(c * soft, 1.0);
}
"#;

const BLOOM_BLUR_SHADER: &str = r#"
@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var tex_sampler: sampler;

struct BlurParams {
    direction: vec4<f32>,
};
@group(1) @binding(0) var<uniform> params: BlurParams;

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOutput {
    let pos = array<vec2<f32>, 3>(vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
    var out: VertexOutput;
    out.clip = vec4<f32>(pos[idx], 0.0, 1.0);
    let uv = pos[idx] * 0.5 + 0.5;
    out.uv = vec2(uv.x, 1.0 - uv.y);
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let dir = params.direction.xy;
    let uv = input.uv;
    // 9-tap Gaussian (sigma ~3.0).
    var result = textureSampleLevel(tex, tex_sampler, uv, 0.0).rgb * 0.227027;
    result += textureSampleLevel(tex, tex_sampler, uv + dir * 1.0, 0.0).rgb * 0.1945946;
    result += textureSampleLevel(tex, tex_sampler, uv - dir * 1.0, 0.0).rgb * 0.1945946;
    result += textureSampleLevel(tex, tex_sampler, uv + dir * 2.0, 0.0).rgb * 0.1216216;
    result += textureSampleLevel(tex, tex_sampler, uv - dir * 2.0, 0.0).rgb * 0.1216216;
    result += textureSampleLevel(tex, tex_sampler, uv + dir * 3.0, 0.0).rgb * 0.054054;
    result += textureSampleLevel(tex, tex_sampler, uv - dir * 3.0, 0.0).rgb * 0.054054;
    result += textureSampleLevel(tex, tex_sampler, uv + dir * 4.0, 0.0).rgb * 0.016216;
    result += textureSampleLevel(tex, tex_sampler, uv - dir * 4.0, 0.0).rgb * 0.016216;
    return vec4<f32>(result, 1.0);
}
"#;

// ============================================================================
// Mesh
// ============================================================================

#[derive(Clone)]
pub struct Mesh {
    pub vertex_buffer: wgpu::Buffer,
    pub index_buffer: wgpu::Buffer,
    pub index_count: u32,
}

const MAX_DYNAMIC_OBJECTS: usize = 64;

// ============================================================================
// Renderer
// ============================================================================

pub struct Renderer {
    _window: Arc<Window>,
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // G-buffer attachments (recreated on resize)
    gb_albedo_tex: wgpu::Texture,
    gb_albedo_view: wgpu::TextureView,
    gb_normal_tex: wgpu::Texture,
    gb_normal_view: wgpu::TextureView,
    gb_position_tex: wgpu::Texture,
    gb_position_view: wgpu::TextureView,
    depth_tex: wgpu::Texture,
    depth_view: wgpu::TextureView,
    hdr_tex: wgpu::Texture,
    hdr_view: wgpu::TextureView,

    // MSAA (geometry pass renders here, auto-resolves into gb_* textures)
    msaa_samples: u32,
    msaa_albedo_tex: wgpu::Texture,
    msaa_albedo_view: wgpu::TextureView,
    msaa_normal_tex: wgpu::Texture,
    msaa_normal_view: wgpu::TextureView,
    msaa_position_tex: wgpu::Texture,
    msaa_position_view: wgpu::TextureView,
    msaa_depth_tex: wgpu::Texture,
    msaa_depth_view: wgpu::TextureView,

    // Shadow map (fixed size)
    shadow_tex: wgpu::Texture,
    shadow_view: wgpu::TextureView,

    // Pipelines
    geometry_pipeline: wgpu::RenderPipeline,
    shadow_pipeline: wgpu::RenderPipeline,
    lighting_pipeline: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,

    // Geometry uniform buffer (per-instance, dynamic offset)
    geometry_uniform_buf: wgpu::Buffer,
    geometry_bind_group: wgpu::BindGroup,
    geometry_uniform_size: u64,
    geometry_alignment: u32,
    geometry_bgl: wgpu::BindGroupLayout,
    mat_layout: wgpu::BindGroupLayout,

    // Shadow uniform buffer (per-instance, dynamic offset)
    shadow_uniform_buf: wgpu::Buffer,
    shadow_bind_group: wgpu::BindGroup,

    // Lighting uniform buffer (one per frame)
    light_uniform_buf: wgpu::Buffer,
    light_bind_group: wgpu::BindGroup,

    // Sky uniform buffer (one per frame)
    sky_uniform_buf: wgpu::Buffer,
    sky_bind_group: wgpu::BindGroup,

    // G-buffer bind group (for lighting pass)
    gbuffer_bind_group: wgpu::BindGroup,

    // Tonemap bind group (HDR texture + sampler)
    tonemap_bind_group: wgpu::BindGroup,

    // Samplers
    gbuffer_sampler: wgpu::Sampler,
    shadow_sampler: wgpu::Sampler,
    hdr_sampler: wgpu::Sampler,

    // Tonemap uniform (exposure + bloom)
    tonemap_uni_buf: wgpu::Buffer,
    tonemap_uni_bg: wgpu::BindGroup,

    // Bloom
    bloom_a_tex: wgpu::Texture,
    bloom_a_view: wgpu::TextureView,
    bloom_b_tex: wgpu::Texture,
    bloom_b_view: wgpu::TextureView,
    bloom_extract_pipeline: wgpu::RenderPipeline,
    bloom_blur_pipeline: wgpu::RenderPipeline,
    bloom_hdr_bg: wgpu::BindGroup,
    bloom_a_bg: wgpu::BindGroup,
    bloom_b_bg: wgpu::BindGroup,
    bloom_dir_buf: wgpu::Buffer,
    bloom_dir_bg: wgpu::BindGroup,

    // Material texture (shared by all objects)
    material_bind_group: wgpu::BindGroup,

    // Meshes
    cube_mesh: Mesh,
    ground_mesh: Mesh,
    meshes: HashMap<String, Mesh>,

    // Imgui
    imgui_context: SendableImGuiContext,
    imgui_renderer: imgui_wgpu::Renderer,

    // Console state
    console_open: bool,
    console_input: String,
    console_history_index: usize,
}

// ============================================================================
// Helper: create a texture + view for the G-buffer
// ============================================================================

fn create_gbuffer_texture(
    device: &wgpu::Device,
    label: &str,
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::Texture, wgpu::TextureView) {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    (tex, view)
}

impl Renderer {
    // ----------------------------------------------------------------
    // Material texture
    // ----------------------------------------------------------------

    fn create_material_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
    ) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
        let size = 64u32;
        let mut pixels = Vec::with_capacity((size * size * 4) as usize);
        for y in 0..size {
            for x in 0..size {
                let is_white = (x / 8 + y / 8) % 2 == 0;
                pixels.extend(if is_white {
                    &[255u8, 255, 255, 255]
                } else {
                    &[64, 64, 64, 255]
                });
            }
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Material Texture"),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Material Sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Material BG Layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Material BG"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        (layout, bind_group)
    }

    // ----------------------------------------------------------------
    // Constructor
    // ----------------------------------------------------------------

    pub async fn new(window: Arc<Window>) -> Self {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });

        let surface = instance.create_surface(window.as_ref()).unwrap();
        let surface: wgpu::Surface<'static> = unsafe { std::mem::transmute(surface) };

        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("Failed to find adapter");

        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: None,
                    required_features: wgpu::Features::empty(),
                    required_limits: wgpu::Limits::downlevel_defaults(),
                    memory_hints: Default::default(),
                    experimental_features: Default::default(),
                    trace: Default::default(),
                },
            )
            .await
            .expect("Failed to request device");

        let size = window.inner_size();
        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .find(|f| f.is_srgb())
            .copied()
            .unwrap_or(surface_caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: surface_caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // --- G-buffer textures ---
        let w = size.width.max(1);
        let h = size.height.max(1);

        let (gb_albedo_tex, gb_albedo_view) =
            create_gbuffer_texture(&device, "GB_Albedo", w, h, GB_ALBEDO_FORMAT, 1);
        let (gb_normal_tex, gb_normal_view) =
            create_gbuffer_texture(&device, "GB_Normal", w, h, GB_NORMAL_FORMAT, 1);
        let (gb_position_tex, gb_position_view) =
            create_gbuffer_texture(&device, "GB_Position", w, h, GB_POSITION_FORMAT, 1);
        let (hdr_tex, hdr_view) = create_gbuffer_texture(&device, "HDR_Output", w, h, HDR_FORMAT, 1);

        let depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // --- MSAA textures (4x) ---
        let msaa_samples = 4u32;
        let (msaa_albedo_tex, msaa_albedo_view) =
            create_gbuffer_texture(&device, "MSAA_Albedo", w, h, GB_ALBEDO_FORMAT, msaa_samples);
        let (msaa_normal_tex, msaa_normal_view) =
            create_gbuffer_texture(&device, "MSAA_Normal", w, h, GB_NORMAL_FORMAT, msaa_samples);
        let (msaa_position_tex, msaa_position_view) =
            create_gbuffer_texture(&device, "MSAA_Position", w, h, GB_POSITION_FORMAT, msaa_samples);
        let msaa_depth_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Depth Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: msaa_samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let msaa_depth_view = msaa_depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Shadow map ---
        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Shadow Map"),
            size: wgpu::Extent3d {
                width: SHADOW_MAP_SIZE,
                height: SHADOW_MAP_SIZE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // --- Built-in meshes ---
        let cube_mesh = Self::create_mesh(&device, CUBE_VERTICES, CUBE_INDICES, "Cube");
        let ground_mesh = Self::create_mesh(&device, GROUND_VERTICES, GROUND_INDICES, "Ground");

        // --- Samplers ---
        let gbuffer_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("GBuffer Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            mipmap_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });

        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Shadow Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Nearest,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let hdr_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("HDR Sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // --- Material texture ---
        let (mat_layout, material_bind_group) =
            Self::create_material_texture(&device, &queue);

        // =============================================================
        // Geometry pipeline
        // =============================================================

        let geometry_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Geometry Shader"),
            source: wgpu::ShaderSource::Wgsl(GEOMETRY_SHADER.into()),
        });

        let geometry_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Geometry BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let geometry_uniform_size = std::mem::size_of::<GeometryUniforms>() as u64;
        let geometry_alignment = device.limits().min_uniform_buffer_offset_alignment as u64;
        let geom_aligned = (geometry_uniform_size + geometry_alignment - 1) / geometry_alignment
            * geometry_alignment;

        let geometry_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Geometry Uniform Buffer"),
            size: geom_aligned * MAX_DYNAMIC_OBJECTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let geometry_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Geometry BG"),
            layout: &geometry_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &geometry_uniform_buf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(geometry_uniform_size),
                }),
            }],
        });

        let geometry_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Geometry Pipeline Layout"),
                bind_group_layouts: &[&geometry_bgl, &mat_layout],
                push_constant_ranges: &[],
            },
        );

        let geometry_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Geometry Pipeline"),
                layout: Some(&geometry_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &geometry_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: msaa_samples,
                    ..Default::default()
                },
                fragment: Some(wgpu::FragmentState {
                    module: &geometry_shader,
                    entry_point: Some("fs_main"),
                    targets: &[
                        Some(wgpu::ColorTargetState {
                            format: GB_ALBEDO_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: GB_NORMAL_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                        Some(wgpu::ColorTargetState {
                            format: GB_POSITION_FORMAT,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        }),
                    ],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            });

        // =============================================================
        // Shadow pipeline
        // =============================================================

        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Shadow Shader"),
            source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });

        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Shadow BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let shadow_uniform_size = std::mem::size_of::<GeometryUniforms>() as u64;
        let shadow_aligned = (shadow_uniform_size + geometry_alignment - 1) / geometry_alignment
            * geometry_alignment;

        let shadow_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Shadow Uniform Buffer"),
            size: shadow_aligned * MAX_DYNAMIC_OBJECTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shadow_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Shadow BG"),
            layout: &shadow_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &shadow_uniform_buf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(shadow_uniform_size),
                }),
            }],
        });

        let shadow_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Shadow Pipeline Layout"),
                bind_group_layouts: &[&shadow_bgl],
                push_constant_ranges: &[],
            },
        );

        let shadow_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Shadow Pipeline"),
                layout: Some(&shadow_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shadow_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Vertex::desc()],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: Some(wgpu::Face::Back),
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: true,
                    depth_compare: wgpu::CompareFunction::Less,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: None, // depth-only
                multiview: None,
                cache: None,
            });

        // =============================================================
        // G-buffer bind group (for lighting pass)
        // =============================================================

        let gbuffer_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("GBuffer BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
                // G-buffer depth texture (for sky detection)
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let gbuffer_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GBuffer BG"),
            layout: &gbuffer_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&gb_albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&gb_normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&gb_position_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&shadow_sampler),
                },
                // G-buffer depth for sky detection
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
            ],
        });

        // =============================================================
        // Lighting pipeline
        // =============================================================

        let lighting_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Lighting Shader"),
            source: wgpu::ShaderSource::Wgsl(LIGHTING_SHADER.into()),
        });

        let fsq_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Fullscreen Quad VS"),
            source: wgpu::ShaderSource::Wgsl(FULLSCREEN_QUAD_VS.into()),
        });

        let light_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Light BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let light_uniform_size = std::mem::size_of::<LightUniforms>() as u64;
        let light_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Light Uniform Buffer"),
            size: light_uniform_size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let light_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Light BG"),
            layout: &light_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &light_uniform_buf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(light_uniform_size),
                }),
            }],
        });

        // --- Sky uniform buffer ---
        let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Sky BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let sky_uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Sky Uniform Buffer"),
            size: std::mem::size_of::<SkyUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sky_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Sky BG"),
            layout: &sky_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &sky_uniform_buf,
                    offset: 0,
                    size: std::num::NonZeroU64::new(std::mem::size_of::<SkyUniforms>() as u64),
                }),
            }],
        });

        let lighting_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Lighting Pipeline Layout"),
                bind_group_layouts: &[&gbuffer_bgl, &light_bgl, &sky_bgl],
                push_constant_ranges: &[],
            },
        );

        let lighting_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Lighting Pipeline"),
                layout: Some(&lighting_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &fsq_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &lighting_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            });

        // =============================================================
        // Tonemap pipeline
        // =============================================================

        let tonemap_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Tonemap Shader"),
            source: wgpu::ShaderSource::Wgsl(TONEMAP_SHADER.into()),
        });

        let tonemap_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Tonemap BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // =============================================================
        // Bloom textures (half-res)
        // =============================================================
        let bw = (w / 2).max(1);
        let bh = (h / 2).max(1);
        let (bloom_a_tex, bloom_a_view) =
            create_gbuffer_texture(&device, "Bloom A", bw, bh, HDR_FORMAT, 1);
        let (bloom_b_tex, bloom_b_view) =
            create_gbuffer_texture(&device, "Bloom B", bw, bh, HDR_FORMAT, 1);

        let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Tonemap BG"),
            layout: &tonemap_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&hdr_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&bloom_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&hdr_sampler),
                },
            ],
        });

        // Tonemap uniform buffer (exposure)
        let tonemap_uni_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tonemap Uniform Buffer"),
            size: std::mem::size_of::<TonemapUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let tonemap_uni_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Tonemap Uniform BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tonemap_uni_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Tonemap Uniform BG"),
            layout: &tonemap_uni_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(
                    wgpu::BufferBinding { buffer: &tonemap_uni_buf, offset: 0, size: None },
                ),
            }],
        });

        let tonemap_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Tonemap Pipeline Layout"),
                bind_group_layouts: &[&tonemap_bgl, &tonemap_uni_bgl],
                push_constant_ranges: &[],
            },
        );

        let tonemap_pipeline =
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Tonemap Pipeline"),
                layout: Some(&tonemap_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &tonemap_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &tonemap_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            });

        // =============================================================
        // Bloom pipelines
        // =============================================================

        let bloom_extract_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Extract Shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_EXTRACT_SHADER.into()),
        });
        let bloom_blur_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Bloom Blur Shader"),
            source: wgpu::ShaderSource::Wgsl(BLOOM_BLUR_SHADER.into()),
        });

        // Shared BGL for texture + sampler (extract and blur both use this).
        let bloom_sample_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bloom Sample BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        // BGL for blur direction uniform.
        let bloom_dir_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bloom Dir BGL"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Extract pipeline: fullscreen, reads HDR, writes half-res bloom_a.
        let bloom_extract_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Bloom Extract Pipeline Layout"),
                bind_group_layouts: &[&bloom_sample_bgl],
                push_constant_ranges: &[],
            },
        );
        let bloom_extract_pipeline = device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("Bloom Extract Pipeline"),
                layout: Some(&bloom_extract_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_extract_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_extract_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            },
        );

        // Blur pipeline: fullscreen, reads one half-res tex, writes the other.
        let bloom_blur_pipeline_layout = device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Bloom Blur Pipeline Layout"),
                bind_group_layouts: &[&bloom_sample_bgl, &bloom_dir_bgl],
                push_constant_ranges: &[],
            },
        );
        let bloom_blur_pipeline = device.create_render_pipeline(
            &wgpu::RenderPipelineDescriptor {
                label: Some("Bloom Blur Pipeline"),
                layout: Some(&bloom_blur_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_blur_shader,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_blur_shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            },
        );

        // Bloom bind groups.
        let bloom_hdr_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom Extract BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&hdr_sampler),
                },
            ],
        });
        let bloom_a_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom A BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&bloom_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&hdr_sampler),
                },
            ],
        });
        let bloom_b_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom B BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&bloom_b_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&hdr_sampler),
                },
            ],
        });

        // Blur direction uniform (initially horizontal).
        let bloom_dir_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bloom Dir Buffer"),
            size: 16, // vec4<f32>
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bloom_dir_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom Dir BG"),
            layout: &bloom_dir_bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &bloom_dir_buf,
                    offset: 0,
                    size: None,
                }),
            }],
        });

        // =============================================================
        // Imgui
        // =============================================================

        let mut imgui_context = imgui::Context::create();
        imgui_context.set_ini_filename(None);
        let imgui_renderer = imgui_wgpu::Renderer::new(
            &mut imgui_context,
            &device,
            &queue,
            imgui_wgpu::RendererConfig {
                texture_format: config.format,
                depth_format: None,
                ..imgui_wgpu::RendererConfig::new()
            },
        );

        let mut meshes = HashMap::new();
        meshes.insert("cube".to_string(), cube_mesh.clone());
        meshes.insert("ground".to_string(), ground_mesh.clone());

        Self {
            _window: window,
            _instance: instance,
            surface,
            device,
            queue,
            config,
            gb_albedo_tex,
            gb_albedo_view,
            gb_normal_tex,
            gb_normal_view,
            gb_position_tex,
            gb_position_view,
            depth_tex,
            depth_view,
            hdr_tex,
            hdr_view,
            msaa_samples,
            msaa_albedo_tex,
            msaa_albedo_view,
            msaa_normal_tex,
            msaa_normal_view,
            msaa_position_tex,
            msaa_position_view,
            msaa_depth_tex,
            msaa_depth_view,
            shadow_tex,
            shadow_view,
            geometry_pipeline,
            shadow_pipeline,
            lighting_pipeline,
            tonemap_pipeline,
            geometry_uniform_buf,
            geometry_bind_group,
            geometry_uniform_size,
            geometry_alignment: geometry_alignment as u32,
            geometry_bgl,
            mat_layout,
            shadow_uniform_buf,
            shadow_bind_group,
            light_uniform_buf,
            light_bind_group,
            sky_uniform_buf,
            sky_bind_group,
            gbuffer_bind_group,
            tonemap_bind_group,
            gbuffer_sampler,
            shadow_sampler,
            hdr_sampler,
            tonemap_uni_buf,
            tonemap_uni_bg,
            bloom_a_tex,
            bloom_a_view,
            bloom_b_tex,
            bloom_b_view,
            bloom_extract_pipeline,
            bloom_blur_pipeline,
            bloom_hdr_bg,
            bloom_a_bg,
            bloom_b_bg,
            bloom_dir_buf,
            bloom_dir_bg,
            material_bind_group,
            cube_mesh,
            ground_mesh,
            meshes,
            imgui_context: SendableImGuiContext(imgui_context),
            imgui_renderer,
            console_open: false,
            console_input: String::new(),
            console_history_index: 0,
        }
    }

    // ----------------------------------------------------------------
    // Mesh helpers
    // ----------------------------------------------------------------

    pub fn create_mesh(
        device: &wgpu::Device,
        vertices: &[Vertex],
        indices: &[u32],
        label: &str,
    ) -> Mesh {
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} VB", label)),
            contents: bytemuck::cast_slice(vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&format!("{} IB", label)),
            contents: bytemuck::cast_slice(indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Mesh {
            vertex_buffer: vb,
            index_buffer: ib,
            index_count: indices.len() as u32,
        }
    }

    pub fn register_mesh(&mut self, name: &str, vertices: &[Vertex], indices: &[u32]) {
        let mesh = Self::create_mesh(&self.device, vertices, indices, name);
        self.meshes.insert(name.to_string(), mesh);
    }

    pub fn load_model(&mut self, path: &str) -> Result<Vec<String>, String> {
        let mesh_data_list = model_loader::load_model(path)?;
        let mut names = Vec::new();
        for md in mesh_data_list {
            let vertices: Vec<Vertex> = md
                .vertices
                .iter()
                .map(|v| Vertex {
                    position: v.position,
                    normal: v.normal,
                    uv: v.uv,
                })
                .collect();
            let name = md.name.clone();
            self.register_mesh(&name, &vertices, &md.indices);
            names.push(name);
        }
        Ok(names)
    }

    // ----------------------------------------------------------------
    // Resize
    // ----------------------------------------------------------------

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);

        let w = width;
        let h = height;

        let recreate = |label: &str, format| -> (wgpu::Texture, wgpu::TextureView) {
            create_gbuffer_texture(&self.device, label, w, h, format, 1)
        };

        (self.gb_albedo_tex, self.gb_albedo_view) =
            recreate("GB_Albedo", GB_ALBEDO_FORMAT);
        (self.gb_normal_tex, self.gb_normal_view) =
            recreate("GB_Normal", GB_NORMAL_FORMAT);
        (self.gb_position_tex, self.gb_position_view) =
            recreate("GB_Position", GB_POSITION_FORMAT);
        (self.hdr_tex, self.hdr_view) = recreate("HDR_Output", HDR_FORMAT);

        // Recreate bloom textures at half-res.
        let bw = (w / 2).max(1);
        let bh = (h / 2).max(1);
        (self.bloom_a_tex, self.bloom_a_view) =
            create_gbuffer_texture(&self.device, "Bloom A", bw, bh, HDR_FORMAT, 1);
        (self.bloom_b_tex, self.bloom_b_view) =
            create_gbuffer_texture(&self.device, "Bloom B", bw, bh, HDR_FORMAT, 1);

        self.depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Depth Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        self.depth_view = self.depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Recreate MSAA textures.
        (self.msaa_albedo_tex, self.msaa_albedo_view) =
            create_gbuffer_texture(&self.device, "MSAA_Albedo", w, h, GB_ALBEDO_FORMAT, self.msaa_samples);
        (self.msaa_normal_tex, self.msaa_normal_view) =
            create_gbuffer_texture(&self.device, "MSAA_Normal", w, h, GB_NORMAL_FORMAT, self.msaa_samples);
        (self.msaa_position_tex, self.msaa_position_view) =
            create_gbuffer_texture(&self.device, "MSAA_Position", w, h, GB_POSITION_FORMAT, self.msaa_samples);
        self.msaa_depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Depth Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: self.msaa_samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.msaa_depth_view = self.msaa_depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Rebuild G-buffer bind group with new views.
        self.gbuffer_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("GBuffer BG"),
            layout: &self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("GBuffer BGL"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: false },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 4,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                        count: None,
                    },
                    // G-buffer depth for sky detection
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Depth,
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            }),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.gb_albedo_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&self.gb_normal_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.gb_position_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::TextureView(&self.shadow_view),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::TextureView(&self.depth_view),
                },
            ],
        });

        // Rebuild tonemap bind group with new HDR + bloom views.
        let tonemap_bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Tonemap BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let tonemap_bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Tonemap BG"),
            layout: &tonemap_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.hdr_sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&self.bloom_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.hdr_sampler),
                },
            ],
        });
        self.tonemap_bind_group = tonemap_bind_group;

        // Rebuild bloom bind groups with new views.
        let bloom_sample_bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Bloom Sample BGL"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        self.bloom_hdr_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom Extract BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.hdr_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.hdr_sampler),
                },
            ],
        });
        self.bloom_a_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom A BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.bloom_a_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.hdr_sampler),
                },
            ],
        });
        self.bloom_b_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Bloom B BG"),
            layout: &bloom_sample_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&self.bloom_b_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.hdr_sampler),
                },
            ],
        });
    }

    // ----------------------------------------------------------------
    // Draw geometry + shadow instances (shared logic)
    // ----------------------------------------------------------------

    /// Write per-instance uniforms and draw into a render pass.
    /// `pipeline` must already be set on the pass. The `buffer` and `bind_group`
    /// are for the geometry/shadow uniforms (dynamic offset).
    fn draw_instances_uniform(
        &self,
        pass: &mut wgpu::RenderPass,
        instances: &[DrawInstance],
        default_mesh: &str,
        transform_matrix: &Mat4,  // camera VP or light VP
        buffer: &wgpu::Buffer,
        bind_group: &wgpu::BindGroup,
        uniform_size: u64,
        alignment: u32,
        offset: &mut u32,
    ) {
        for instance in instances {
            let mesh_name = instance.mesh_name.as_deref().unwrap_or(default_mesh);
            let mesh = match self.meshes.get(mesh_name) {
                Some(m) => m,
                None => {
                    log::warn!("Mesh '{mesh_name}' not found, using '{default_mesh}'");
                    if default_mesh == "ground" {
                        &self.ground_mesh
                    } else {
                        &self.cube_mesh
                    }
                }
            };

            let model = Mat4::from_translation(instance.transform.position)
                * Mat4::from_quat(instance.transform.rotation)
                * Mat4::from_scale(instance.transform.scale);
            let u = GeometryUniforms {
                model: model.to_cols_array_2d(),
                view_proj: transform_matrix.to_cols_array_2d(),
            };
            self.queue
                .write_buffer(buffer, *offset as u64, bytemuck::cast_slice(&[u]));
            pass.set_bind_group(0, bind_group, &[*offset]);
            pass.set_vertex_buffer(0, mesh.vertex_buffer.slice(..));
            pass.set_index_buffer(mesh.index_buffer.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
            *offset = (*offset + uniform_size as u32 + alignment - 1) / alignment * alignment;
        }
    }

    fn draw_instances_geometry(
        &self,
        pass: &mut wgpu::RenderPass,
        instances: &[DrawInstance],
        default_mesh: &str,
        view_proj: &Mat4,
        offset: &mut u32,
    ) {
        pass.set_pipeline(&self.geometry_pipeline);
        pass.set_bind_group(1, &self.material_bind_group, &[]);
        self.draw_instances_uniform(
            pass,
            instances,
            default_mesh,
            view_proj,
            &self.geometry_uniform_buf,
            &self.geometry_bind_group,
            self.geometry_uniform_size,
            self.geometry_alignment,
            offset,
        );
    }

    fn draw_instances_shadow(
        &self,
        pass: &mut wgpu::RenderPass,
        instances: &[DrawInstance],
        default_mesh: &str,
        light_vp: &Mat4,
        offset: &mut u32,
    ) {
        pass.set_pipeline(&self.shadow_pipeline);
        self.draw_instances_uniform(
            pass,
            instances,
            default_mesh,
            light_vp,
            &self.shadow_uniform_buf,
            &self.shadow_bind_group,
            std::mem::size_of::<GeometryUniforms>() as u64,
            self.geometry_alignment,
            offset,
        );
    }

    // ----------------------------------------------------------------
    // Main render entry point
    // ----------------------------------------------------------------

    /// Recreate MSAA textures and geometry pipeline when sample count changes.
    fn recreate_msaa(&mut self, new_samples: u32) {
        let w = self.config.width;
        let h = self.config.height;

        (self.msaa_albedo_tex, self.msaa_albedo_view) =
            create_gbuffer_texture(&self.device, "MSAA_Albedo", w, h, GB_ALBEDO_FORMAT, new_samples);
        (self.msaa_normal_tex, self.msaa_normal_view) =
            create_gbuffer_texture(&self.device, "MSAA_Normal", w, h, GB_NORMAL_FORMAT, new_samples);
        (self.msaa_position_tex, self.msaa_position_view) =
            create_gbuffer_texture(&self.device, "MSAA_Position", w, h, GB_POSITION_FORMAT, new_samples);
        self.msaa_depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("MSAA Depth Texture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: new_samples,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        self.msaa_depth_view = self.msaa_depth_tex.create_view(&wgpu::TextureViewDescriptor::default());

        // Recreate geometry pipeline with new multisample count.
        let geometry_shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Geometry Shader"),
            source: wgpu::ShaderSource::Wgsl(GEOMETRY_SHADER.into()),
        });
        let geometry_pipeline_layout = self.device.create_pipeline_layout(
            &wgpu::PipelineLayoutDescriptor {
                label: Some("Geometry Pipeline Layout"),
                bind_group_layouts: &[&self.geometry_bgl, &self.mat_layout],
                push_constant_ranges: &[],
            },
        );
        self.geometry_pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("Geometry Pipeline"),
            layout: Some(&geometry_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &geometry_shader,
                entry_point: Some("vs_main"),
                buffers: &[Vertex::desc()],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: new_samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &geometry_shader,
                entry_point: Some("fs_main"),
                targets: &[
                    Some(wgpu::ColorTargetState {
                        format: GB_ALBEDO_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: GB_NORMAL_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                    Some(wgpu::ColorTargetState {
                        format: GB_POSITION_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    }),
                ],
                compilation_options: Default::default(),
            }),
            cache: None,
            multiview: None,
        });

        self.msaa_samples = new_samples;
    }

    pub fn render(
        &mut self,
        frame: &RenderFrame,
        input_events: &[RenderInputEvent],
    ) -> Option<String> {
        self.console_open = frame.console.open;

        for event in input_events {
            match event {
                RenderInputEvent::MousePos(x, y) => {
                    self.imgui_context.0.io_mut().mouse_pos = [*x, *y];
                }
                RenderInputEvent::MouseButton(btn, pressed) => {
                    self.imgui_context
                        .0
                        .io_mut()
                        .add_mouse_button_event(*btn, *pressed);
                }
                RenderInputEvent::Key(key, pressed) => {
                    self.imgui_context.0.io_mut().add_key_event(*key, *pressed);
                }
                RenderInputEvent::Char(c) => {
                    self.imgui_context.0.io_mut().add_input_character(*c);
                }
                RenderInputEvent::Resize { width, height } => {
                    if *width != self.config.width || *height != self.config.height {
                        self.resize(*width, *height);
                    }
                }
            }
        }

        if frame.width != self.config.width || frame.height != self.config.height {
            self.resize(frame.width, frame.height);
        }

        // Check for MSAA sample count change.
        let desired_msaa = if frame.msaa_samples == 0 { 1 } else { frame.msaa_samples };
        if desired_msaa != self.msaa_samples {
            self.recreate_msaa(desired_msaa);
        }

        self.imgui_context.0.io_mut().display_size = [frame.width as f32, frame.height as f32];

        let surface_texture = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(wgpu::SurfaceError::Timeout | wgpu::SurfaceError::Outdated) => return None,
            Err(e) => {
                log::error!("Surface error: {e}");
                return None;
            }
        };

        let surface_view =
            surface_texture
                .texture
                .create_view(&wgpu::TextureViewDescriptor::default());
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("Deferred Command Encoder"),
            });

        let mut pending_cmd: Option<String> = None;

        // ---------------------------------------------------------
        // 1. Shadow pass — depth-only from light's perspective
        // ---------------------------------------------------------
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Shadow Pass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut offset = 0;
            self.draw_instances_shadow(
                &mut pass,
                &frame.ground_instances,
                "ground",
                &frame.sun_view_proj,
                &mut offset,
            );
            if let Some(ref instance) = frame.player_instance {
                self.draw_instances_shadow(
                    &mut pass,
                    std::slice::from_ref(instance),
                    "cube",
                    &frame.sun_view_proj,
                    &mut offset,
                );
            }
            self.draw_instances_shadow(
                &mut pass,
                &frame.dynamic_instances,
                "cube",
                &frame.sun_view_proj,
                &mut offset,
            );
        }

        // ---------------------------------------------------------
        // 2. Geometry pass — write G-buffer (MSAA resolve into gb_* textures)
        // ---------------------------------------------------------
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Geometry Pass"),
                color_attachments: &[
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.msaa_albedo_view,
                        depth_slice: None,
                        resolve_target: Some(&self.gb_albedo_view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.msaa_normal_view,
                        depth_slice: None,
                        resolve_target: Some(&self.gb_normal_view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                    Some(wgpu::RenderPassColorAttachment {
                        view: &self.msaa_position_view,
                        depth_slice: None,
                        resolve_target: Some(&self.gb_position_view),
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    }),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.msaa_depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            let mut offset = 0;
            self.draw_instances_geometry(
                &mut pass,
                &frame.ground_instances,
                "ground",
                &frame.view_proj,
                &mut offset,
            );
            if let Some(ref instance) = frame.player_instance {
                self.draw_instances_geometry(
                    &mut pass,
                    std::slice::from_ref(instance),
                    "cube",
                    &frame.view_proj,
                    &mut offset,
                );
            }
            self.draw_instances_geometry(
                &mut pass,
                &frame.dynamic_instances,
                "cube",
                &frame.view_proj,
                &mut offset,
            );
        }

        // ---------------------------------------------------------
        // 3. Lighting pass — full-screen quad, reads G-buffer
        // ---------------------------------------------------------
        {
            // Write light uniforms.
            let inv_proj = frame.view_proj.inverse();
            let light_u = LightUniforms {
                light_dir: [
                    frame.sun_light.direction.x,
                    frame.sun_light.direction.y,
                    frame.sun_light.direction.z,
                    0.0,
                ],
                light_color: [
                    frame.sun_light.color.x * frame.sun_light.intensity,
                    frame.sun_light.color.y * frame.sun_light.intensity,
                    frame.sun_light.color.z * frame.sun_light.intensity,
                    0.0,
                ],
                ambient: [
                    frame.ambient.x,
                    frame.ambient.y,
                    frame.ambient.z,
                    0.0,
                ],
                camera_pos: [
                    frame.camera_pos.x,
                    frame.camera_pos.y,
                    frame.camera_pos.z,
                    0.0,
                ],
                light_view_proj: frame.sun_view_proj.to_cols_array_2d(),
                inv_proj: inv_proj.to_cols_array_2d(),
            };
            self.queue.write_buffer(
                &self.light_uniform_buf,
                0,
                bytemuck::cast_slice(&[light_u]),
            );

            // Write sky uniforms.
            let sky_u = SkyUniforms {
                sky_color: [
                    frame.sky.sky_color.x,
                    frame.sky.sky_color.y,
                    frame.sky.sky_color.z,
                    0.0,
                ],
                sky_brightness: frame.sky.brightness,
                sky_intensity: frame.sky.sky_intensity,
                screen_w: frame.width as f32,
                screen_h: frame.height as f32,
                _pad: [0.0; 4],
            };
            self.queue.write_buffer(
                &self.sky_uniform_buf,
                0,
                bytemuck::cast_slice(&[sky_u]),
            );

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Lighting Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.hdr_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 0.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.lighting_pipeline);
            pass.set_bind_group(0, &self.gbuffer_bind_group, &[]);
            pass.set_bind_group(1, &self.light_bind_group, &[]);
            pass.set_bind_group(2, &self.sky_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }

        // ---------------------------------------------------------
        // 3.5. Bloom — extract bright + gaussian blur at half-res
        // ---------------------------------------------------------
        {
            let bw = (frame.width / 2).max(1) as f32;
            let bh = (frame.height / 2).max(1) as f32;
            const BLOOM_ITERS: u32 = 1;

            // Extract: HDR → bloom_a
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("Bloom Extract"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.bloom_a_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color {
                                r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                            }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                pass.set_pipeline(&self.bloom_extract_pipeline);
                pass.set_bind_group(0, &self.bloom_hdr_bg, &[]);
                pass.draw(0..3, 0..1);
            }

            // Blur iterations: ping-pong A ↔ B.
            for _ in 0..BLOOM_ITERS {
                // Horizontal: bloom_a → bloom_b
                self.queue.write_buffer(
                    &self.bloom_dir_buf,
                    0,
                    bytemuck::cast_slice(&[[1.0 / bw, 0.0, 0.0, 0.0f32]]),
                );
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Bloom Blur H"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.bloom_b_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&self.bloom_blur_pipeline);
                    pass.set_bind_group(0, &self.bloom_a_bg, &[]);
                    pass.set_bind_group(1, &self.bloom_dir_bg, &[]);
                    pass.draw(0..3, 0..1);
                }

                // Vertical: bloom_b → bloom_a
                self.queue.write_buffer(
                    &self.bloom_dir_buf,
                    0,
                    bytemuck::cast_slice(&[[0.0, 1.0 / bh, 0.0, 0.0f32]]),
                );
                {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("Bloom Blur V"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.bloom_a_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color {
                                    r: 0.0, g: 0.0, b: 0.0, a: 0.0,
                                }),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                    });
                    pass.set_pipeline(&self.bloom_blur_pipeline);
                    pass.set_bind_group(0, &self.bloom_b_bg, &[]);
                    pass.set_bind_group(1, &self.bloom_dir_bg, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
        }

        // ---------------------------------------------------------
        // 4. Tonemap pass — Reinhard + gamma to swapchain
        // ---------------------------------------------------------
        {
            // Write exposure + bloom uniforms.
            let tonemap_u = TonemapUniforms {
                exposure: frame.exposure,
                bloom_intensity: frame.bloom_intensity,
                _pad: [0.0; 2],
            };
            self.queue.write_buffer(
                &self.tonemap_uni_buf,
                0,
                bytemuck::cast_slice(&[tonemap_u]),
            );

            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Tonemap Pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &surface_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0,
                            g: 0.0,
                            b: 0.0,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            pass.set_pipeline(&self.tonemap_pipeline);
            pass.set_bind_group(0, &self.tonemap_bind_group, &[]);
            pass.set_bind_group(1, &self.tonemap_uni_bg, &[]);
            pass.draw(0..3, 0..1);

            // Imgui overlay renders on top of the tonemapped result.
            let ui = self.imgui_context.0.frame();

            Self::draw_console(
                &ui,
                &mut self.console_open,
                &frame.console,
                &mut self.console_input,
                &mut self.console_history_index,
                &mut pending_cmd,
            );
            Self::draw_ui_labels(&ui, &frame.ui_labels);

            let draw_data = self.imgui_context.0.render();
            self.imgui_renderer
                .render(draw_data, &self.queue, &self.device, &mut pass)
                .expect("imgui render failed");
        }

        self.queue.submit([encoder.finish()]);
        surface_texture.present();

        pending_cmd
    }

    // ----------------------------------------------------------------
    // Helper: tonemap bind group rebuild (for resize)
    // ----------------------------------------------------------------

    /// Rebuild the G-buffer bind group — called from resize.
    /// We do this inline in resize() rather than as a separate method.

    // ----------------------------------------------------------------
    // Imgui helpers
    // ----------------------------------------------------------------

    fn draw_console(
        ui: &imgui::Ui,
        console_open: &mut bool,
        console: &ConsoleState,
        console_input: &mut String,
        history_index: &mut usize,
        pending_cmd: &mut Option<String>,
    ) {
        if !*console_open {
            return;
        }

        ui.window("Console")
            .position([10.0, 10.0], imgui::Condition::FirstUseEver)
            .size([500.0, 350.0], imgui::Condition::FirstUseEver)
            .opened(console_open)
            .build(|| {
                ui.child_window("ConsoleLog")
                    .size([-1.0, -1.0])
                    .build(|| {
                        let at_bottom =
                            ui.scroll_y() >= ui.scroll_max_y().max(0.0) - 5.0;
                        for msg in &console.log {
                            ui.text_wrapped(msg);
                        }
                        if pending_cmd.is_some() || at_bottom {
                            ui.set_scroll_here_y();
                        }
                    });

                ui.separator();
                if ui.is_window_appearing() {
                    ui.set_keyboard_focus_here();
                }
                ui.input_text("##console_input", console_input)
                    .enter_returns_true(true)
                    .build();
                ui.same_line();
                if ui.button("Submit") {
                    if !console_input.trim().is_empty() {
                        let cmd = console_input.trim().to_string();
                        console_input.clear();
                        *pending_cmd = Some(cmd);
                    }
                }

                if ui.is_key_pressed(imgui::Key::Enter)
                    && !console_input.trim().is_empty()
                {
                    let cmd = console_input.trim().to_string();
                    console_input.clear();
                    *pending_cmd = Some(cmd);
                }

                if ui.is_key_pressed(imgui::Key::UpArrow)
                    && !ui.is_key_down(imgui::Key::LeftCtrl)
                {
                    if !console.history.is_empty() {
                        if *history_index > 0 {
                            *history_index -= 1;
                        }
                        if let Some(entry) = console.history.get(*history_index) {
                            *console_input = entry.clone();
                        }
                    }
                }
                if ui.is_key_pressed(imgui::Key::DownArrow)
                    && !ui.is_key_down(imgui::Key::LeftCtrl)
                {
                    if *history_index < console.history.len().saturating_sub(1) {
                        *history_index += 1;
                        if let Some(entry) = console.history.get(*history_index) {
                            *console_input = entry.clone();
                        }
                    } else {
                        *history_index = console.history.len();
                        console_input.clear();
                    }
                }
            });
    }

    fn draw_ui_labels(ui: &imgui::Ui, labels: &[String]) {
        if labels.is_empty() {
            return;
        }

        let [w, _h] = ui.io().display_size;
        ui.window("Lua UI")
            .position([w - 320.0, 10.0], imgui::Condition::Always)
            .size([300.0, 0.0], imgui::Condition::Always)
            .title_bar(true)
            .resizable(false)
            .movable(true)
            .build(|| {
                for label in labels {
                    ui.text_wrapped(label);
                }
            });
    }
}
