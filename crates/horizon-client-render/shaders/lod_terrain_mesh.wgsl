// GPU-driven far terrain: a task shader culls and foveates LOD chunks, then a
// mesh shader emits their quads straight from the tile vertex stream. The
// vertex data is the same `TerrainVertex` stream the CPU mesher already makes,
// so the result matches the ordinary far-terrain draw exactly.
enable wgpu_mesh_shader;

// terrain.wgsl's group 0, truncated to the fields far terrain reads.
struct Globals {
    view_proj: mat4x4<f32>,
    fog_color: vec4<f32>,
    fog: vec4<f32>,
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var sprites: texture_2d_array<f32>;
@group(0) @binding(2) var sprite_sampler: sampler;
@group(0) @binding(3) var lightmap: texture_2d<f32>;
@group(0) @binding(4) var lightmap_sampler: sampler;
@group(0) @binding(5) var sprites_1: texture_2d_array<f32>;
@group(0) @binding(6) var sprites_2: texture_2d_array<f32>;
@group(0) @binding(7) var sprites_3: texture_2d_array<f32>;
@group(0) @binding(8) var sprites_4: texture_2d_array<f32>;
@group(0) @binding(9) var sprites_5: texture_2d_array<f32>;
@group(0) @binding(10) var sprites_6: texture_2d_array<f32>;
@group(0) @binding(11) var sprites_7: texture_2d_array<f32>;

override SPRITE_ARRAYS: u32 = 1u;
override SPRITE_SPLIT: u32 = 2048u;

fn sprite(uv: vec2<f32>, layer: u32) -> vec4<f32> {
    if SPRITE_ARRAYS == 1u {
        return textureSample(sprites, sprite_sampler, uv, i32(layer));
    }
    let dx = dpdx(uv);
    let dy = dpdy(uv);
    let l = i32(layer % SPRITE_SPLIT);
    switch layer / SPRITE_SPLIT {
        case 0u: { return textureSampleGrad(sprites, sprite_sampler, uv, l, dx, dy); }
        case 1u: { return textureSampleGrad(sprites_1, sprite_sampler, uv, l, dx, dy); }
        case 2u: { return textureSampleGrad(sprites_2, sprite_sampler, uv, l, dx, dy); }
        case 3u: { return textureSampleGrad(sprites_3, sprite_sampler, uv, l, dx, dy); }
        case 4u: { return textureSampleGrad(sprites_4, sprite_sampler, uv, l, dx, dy); }
        case 5u: { return textureSampleGrad(sprites_5, sprite_sampler, uv, l, dx, dy); }
        case 6u: { return textureSampleGrad(sprites_6, sprite_sampler, uv, l, dx, dy); }
        default: { return textureSampleGrad(sprites_7, sprite_sampler, uv, l, dx, dy); }
    }
}

struct GpuChunkHeader {
    // World tile origin; w is the tile's fixed-point UV step (1/32).
    origin: vec4<f32>,
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,
    vertex_offset: u32,
    quad_count: u32,
    // Bit 0: the chunk is inside the foveation cone (hysteresis state).
    flags: u32,
    _pad: u32,
}

struct LodGlobals {
    // xyz world camera, w unused.
    camera: vec4<f32>,
    // xz forward look, z cos(enter), w cos(exit).
    forward: vec4<f32>,
    // x meshlet debug coloring, y resident chunk count.
    params: vec4<u32>,
}

@group(1) @binding(0) var<storage, read_write> chunks: array<GpuChunkHeader>;
@group(1) @binding(1) var<storage, read> verts: array<u32>;
@group(1) @binding(2) var<uniform> lod: LodGlobals;

struct TaskPayload {
    chunk: u32,
    first_quad: u32,
    foveated: u32,
    _pad: u32,
}
var<task_payload> payload: TaskPayload;

struct TerrainOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) layer: u32,
    @location(2) color: vec4<f32>,
    @location(3) spherical: f32,
    @location(4) cylindrical: f32,
    @location(5) @interpolate(flat) emission: u32,
    @location(6) @interpolate(flat) meshlet: u32,
}

fn sample_lightmap(sky: f32, block: f32) -> vec4<f32> {
    let uv = vec2(block, sky) * 16.0;
    return textureSampleLevel(lightmap, lightmap_sampler,
        clamp(uv / 256.0 + 0.5 / 16.0, vec2(0.5 / 16.0), vec2(15.5 / 16.0)), 0.0);
}

fn terrain_vertex(position: vec3<f32>, uv: vec2<f32>, layer: u32, color: vec4<f32>,
    light: vec2<u32>, origin: vec3<f32>) -> TerrainOut {
    let pos = position + origin;
    var out: TerrainOut;
    out.clip = globals.view_proj * vec4(pos, 1.0);
    out.uv = uv;
    out.layer = layer;
    out.color = vec4(color.rgb, 1.0) * sample_lightmap(f32(light.x) / 16.0, f32(light.y) / 16.0);
    out.emission = 15u - (u32(round(color.a * 255.0)) & 15u);
    out.spherical = length(pos);
    out.cylindrical = max(length(pos.xz), abs(pos.y));
    return out;
}

fn linear_fog(d: f32, start: f32, end: f32) -> f32 {
    if d <= start { return 0.0; }
    if d >= end { return 1.0; }
    return (d - start) / (end - start);
}

fn apply_fog(color: vec4<f32>, spherical: f32, cylindrical: f32) -> vec4<f32> {
    let f = max(linear_fog(spherical, globals.fog.x, globals.fog.y),
                linear_fog(cylindrical, globals.fog.z, globals.fog.w));
    return vec4(mix(color.rgb, globals.fog_color.rgb, f * globals.fog_color.a), color.a);
}

struct LoadedVertex {
    pos: vec3<f32>,
    uv: vec2<u32>,
    layer: u32,
    light: vec2<u32>,
    color: u32,
}

fn load_vertex(index: u32) -> LoadedVertex {
    let b = index * 6u;
    let uv_layer_light = verts[b + 4u];
    let light = uv_layer_light >> 16u;
    return LoadedVertex(
        vec3<f32>(bitcast<f32>(verts[b]), bitcast<f32>(verts[b + 1u]), bitcast<f32>(verts[b + 2u])),
        vec2<u32>(verts[b + 3u] & 0xffffu, verts[b + 3u] >> 16u),
        uv_layer_light & 0xffffu,
        vec2<u32>(light & 0xffu, light >> 8u),
        verts[b + 5u],
    );
}

fn unpack_color(c: u32) -> vec4<f32> {
    return vec4<f32>(
        f32(c & 0xffu),
        f32((c >> 8u) & 0xffu),
        f32((c >> 16u) & 0xffu),
        f32((c >> 24u) & 0xffu),
    ) / 255.0;
}

// lod.forward.z/w: 17 and 23 degrees. They enter and exit the 20-degree
// foveation cone with a 3-degree deadband, so micro-panning cannot flip a
// chunk every frame.

fn clip_outside(clip: vec4<f32>) -> vec3<bool> {
    return vec3<bool>(
        clip.x < -clip.w || clip.x > clip.w,
        clip.y < -clip.w || clip.y > clip.w,
        clip.z < 0.0 || clip.z > clip.w,
    );
}

@task @payload(payload) @workgroup_size(1, 1, 1)
fn ts_lod(@builtin(workgroup_id) id: vec3<u32>) -> @builtin(mesh_task_size) vec3<u32> {
    let ci = id.x;
    if ci >= lod.params.y {
        return vec3<u32>(0u, 1u, 1u);
    }
    let chunk = chunks[ci];
    if chunk.quad_count == 0u {
        return vec3<u32>(0u, 1u, 1u);
    }
    let camera = lod.camera.xyz;
    // Conservative frustum test over the eight bounds corners.
    var out_x = true;
    var out_y = true;
    var out_z = true;
    for (var i = 0u; i < 8u; i++) {
        let corner = vec3<f32>(
            select(chunk.bounds_min.x, chunk.bounds_max.x, (i & 1u) != 0u),
            select(chunk.bounds_min.y, chunk.bounds_max.y, (i & 2u) != 0u),
            select(chunk.bounds_min.z, chunk.bounds_max.z, (i & 4u) != 0u),
        );
        let clip = globals.view_proj * vec4(corner - camera, 1.0);
        let outside = clip_outside(clip);
        out_x = out_x && outside.x;
        out_y = out_y && outside.y;
        out_z = out_z && outside.z;
    }
    if out_x || out_y || out_z {
        return vec3<u32>(0u, 1u, 1u);
    }

    let center = (chunk.bounds_min.xyz + chunk.bounds_max.xyz) * 0.5;
    let offset = center.xz - camera.xz;
    let distance = length(offset);
    let forward = normalize(lod.forward.xz);
    let cos_angle = select(1.0, dot(offset / max(distance, 0.0001), forward), distance > 0.0001);
    let was_foveated = (chunk.flags & 1u) != 0u;
    var foveated = cos_angle >= lod.forward.z;
    if was_foveated {
        foveated = cos_angle >= lod.forward.w;
    }
    chunks[ci].flags = select(chunk.flags & ~1u, chunk.flags | 1u, foveated);

    let meshlets = (chunk.quad_count + 31u) / 32u;
    payload = TaskPayload(ci, 0u, select(0u, 1u, foveated), 0u);
    return vec3<u32>(meshlets, 1u, 1u);
}

struct PrimitiveOut {
    @builtin(triangle_indices) indices: vec3<u32>,
}

struct MeshOut {
    @builtin(vertex_count) vertex_count: u32,
    @builtin(primitive_count) primitive_count: u32,
    @builtin(vertices) vertices: array<TerrainOut, 128>,
    @builtin(primitives) primitives: array<PrimitiveOut, 64>,
}

var<workgroup> out_mesh: MeshOut;

@mesh(out_mesh) @payload(payload) @workgroup_size(32, 1, 1)
fn ms_lod(
    @builtin(workgroup_id) wgid: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let chunk = chunks[payload.chunk];
    let meshlet = wgid.x;
    let first_quad = meshlet * 32u;
    if first_quad >= chunk.quad_count {
        if lane == 0u {
            out_mesh.vertex_count = 0u;
            out_mesh.primitive_count = 0u;
        }
        return;
    }
    let live = min(32u, chunk.quad_count - first_quad);
    if lane == 0u {
        out_mesh.vertex_count = live * 4u;
        out_mesh.primitive_count = live * 2u;
    }
    workgroupBarrier();
    if lane >= live {
        return;
    }

    let quad = first_quad + lane;
    let base = chunk.vertex_offset + quad * 4u;
    let origin = chunk.origin.xyz - lod.camera.xyz;
    for (var corner = 0u; corner < 4u; corner++) {
        let v = load_vertex(base + corner);
        var out = terrain_vertex(
            v.pos,
            vec2<f32>(v.uv) * chunk.origin.w,
            v.layer,
            unpack_color(v.color),
            v.light,
            origin,
        );
        out.meshlet = meshlet;
        out_mesh.vertices[lane * 4u + corner] = out;
    }
    out_mesh.primitives[lane * 2u].indices =
        vec3<u32>(lane * 4u, lane * 4u + 1u, lane * 4u + 2u);
    out_mesh.primitives[lane * 2u + 1u].indices =
        vec3<u32>(lane * 4u, lane * 4u + 2u, lane * 4u + 3u);
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let p = abs(fract(vec3<f32>(h) + k / 6.0) * 6.0 - 3.0);
    return v * mix(vec3<f32>(1.0), clamp(p - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)), s);
}

@fragment
fn fs_lod(in: TerrainOut) -> @location(0) vec4<f32> {
    var color = sprite(in.uv, in.layer & 0x7fffu) * in.color;
    if color.a < 0.5 && (in.layer & 0x8000u) == 0u {
        discard;
    }
    if lod.params.x != 0u {
        let hue = f32((in.meshlet * 2654435761u) % 360u) / 360.0;
        color = vec4<f32>(mix(color.rgb, hsv_to_rgb(hue, 0.75, 1.0), 0.65), color.a);
    }
    let emissive = select(1.0, 0.7 * f32(4u - min(in.emission, 4u)) / 4.0, in.emission > 0u);
    return apply_fog(vec4(color.rgb, emissive), in.spherical, in.cylindrical);
}
