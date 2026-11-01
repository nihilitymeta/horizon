// GPU-driven far terrain: task and mesh shaders over the solid quads of the
// resident far tiles, in the terrain arena where the ordinary draws read
// them. This file is appended to terrain.wgsl (gfx/lod/pipeline.rs), so a
// quad comes out of `terrain_vertex` and `fs_solid` exactly as the vertex
// pipeline draws it; what moves to the GPU is the per-tile work the renderer
// does on the CPU for the vertex path each frame: the frustum test
// (`Bounds::visible`), the choice of which buckets of quads can face the eye
// (`Sides::visible_from`) and the draw list.
//
// A task workgroup takes one tile of one arena page, or one chunk of a big
// tile (`MESHLETS_PER_TASK` meshlets of 32 quads): it culls the tile and
// counts its meshlets over the buckets the eye can see, and dispatches its
// chunk's. The fan-out of a task workgroup is what RADV's task-to-mesh ring
// takes in stride: 32 tiles' worth from one workgroup ran four times slower
// than one tile's. A mesh workgroup maps its 32 quads through the seen
// buckets to their places in the arena, and emits them.

struct GpuTile {
    // Tile-relative box of its solid quads (gfx/lod.rs's `Bounds`).
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,
    // Its first vertex in the arena page, and its solid quads.
    first_vertex: u32,
    quads: u32,
    // Occlusion (gfx/cull.rs's two phases, for far tiles): the stamp of the
    // last frame whose box test saw it (`FRESH` for a tile not tested yet),
    // and of the last frame phase 1 drew it.
    seen: u32,
    drawn: u32,
    // mesher/sides.rs's `Sides`: where each bucket ends, and its plane.
    ends: array<u32, 7>,
    _pad2: u32,
    planes: array<f32, 7>,
    _pad3: f32,
}

// One dispatch: a pass (the world, or the water mirror) over one page.
struct LodDraw {
    // The culling projection, camera-relative (`Renderer::render`'s `cull_proj`).
    view_proj: mat4x4<f32>,
    // xyz: the camera-relative eye of `Sides::visible_from`; w > 0 when the
    // side buckets are culled at all (`Renderer::sides_on`).
    eye: vec4<f32>,
    // x: the water mirror's eye, straight below, as a camera-relative height;
    // y > 0 when there is one (`facing_runs_from`'s `low`).
    low: vec4<f32>,
    // x: the first entry of this page's tiles in `list`, y: how many,
    // z: bit 0 colour each meshlet (F3 + M), bit 1 the frame culls (two
    // phases), bit 2 this is phase 2; w: the frame's stamp.
    params: vec4<u32>,
}

@group(1) @binding(0) var<storage, read_write> tiles: array<GpuTile>;
// The arena page's `TerrainVertex`s as words: position, packed UV, layer and
// light, colour (gfx.rs's `TERRAIN_ATTRIBUTES`).
@group(1) @binding(1) var<storage, read> verts: array<u32>;
// Each tile slot's instance (gfx/lod.rs's `instance`): its camera-relative
// origin and, in w, its UV step and fade code; w = 0 for one not drawn.
@group(1) @binding(2) var<storage, read> instances: array<vec4<f32>>;
// This frame's work by page: a tile's slot << 8 | the index of its chunk of
// `MESHLETS_PER_TASK` meshlets.
@group(1) @binding(3) var<storage, read> list: array<u32>;
@group(1) @binding(4) var<uniform> draw: LodDraw;
// The frame's counts (gfx/lod/pipeline.rs's `Counts`): tiles phase 1 and
// phase 2 dispatched, boxes tested, boxes seen.
@group(1) @binding(5) var<storage, read_write> counts: array<atomic<u32>, 4>;

const QUADS_PER_MESHLET: u32 = 32u;
// The most meshlets one task workgroup dispatches (gfx/lod/pipeline.rs's
// `MESHLETS_PER_TASK`).
const MESHLETS_PER_TASK: u32 = 64u;
// A tile seen within this many frames is phase 1's, and tested again every
// so many frames (cull.rs's `RETEST`); one not tested yet counts as seen.
const RETEST: u32 = 8u;
const FRESH: u32 = 0xffffffffu;
// A box over more than this share of the screen counts as seen (cull.rs's `BIG`).
const BIG: f32 = 0.25;

fn culling() -> bool {
    return (draw.params.z & 2u) != 0u;
}
fn phase2() -> bool {
    return (draw.params.z & 4u) != 0u;
}
// Whether the box test saw the tile in the last `RETEST` frames: phase 1's.
fn seen_lately(slot: u32) -> bool {
    let seen = tiles[slot].seen;
    let stamp = draw.params.w;
    return seen == FRESH || (seen <= stamp && stamp - seen <= RETEST);
}
// mesher/sides.rs's `MARGIN`.
const SIDE_MARGIN: f32 = 1.0 / 16.0;

struct TaskPayload {
    slot: u32,
    // The buckets the eye sees (7 bits).
    mask: u32,
    // The chunk's first meshlet among the tile's.
    first_meshlet: u32,
}
var<task_payload> payload: TaskPayload;

// Whether the tile's box at the camera-relative `origin` is at least partly
// in view: gfx.rs's `Frustum::intersects` over the planes of `frustum`, the
// box outside when its nearest corner to any plane lies behind it.
fn in_view(slot: u32, origin: vec3<f32>) -> bool {
    let min = tiles[slot].bounds_min.xyz + origin;
    let max = tiles[slot].bounds_max.xyz + origin;
    let m = draw.view_proj;
    let r0 = vec4(m[0][0], m[1][0], m[2][0], m[3][0]);
    let r1 = vec4(m[0][1], m[1][1], m[2][1], m[3][1]);
    let r2 = vec4(m[0][2], m[1][2], m[2][2], m[3][2]);
    let r3 = vec4(m[0][3], m[1][3], m[2][3], m[3][3]);
    // No local array: naga gives one a type with an ArrayStride decoration,
    // which SPIR-V forbids in the Function storage class.
    return !(behind(r3 + r0, min, max) || behind(r3 - r0, min, max)
        || behind(r3 + r1, min, max) || behind(r3 - r1, min, max)
        || behind(r2, min, max) || behind(r3 - r2, min, max));
}

// Whether the box lies wholly behind the plane: its nearest corner does.
fn behind(p: vec4<f32>, min: vec3<f32>, max: vec3<f32>) -> bool {
    let v = select(min, max, p.xyz >= vec3(0.0));
    return dot(p.xyz, v) + p.w < 0.0;
}

// The buckets of the tile's quads the eye can see the front of, as a 7-bit
// mask in mesher/sides.rs's `BUCKETS` order: +X, +Z, other, +Y, -Z, -X, -Y
// (`Sides::visible_from`). `origin`: the tile's camera-relative origin.
fn seen_buckets(slot: u32, origin: vec3<f32>) -> u32 {
    if draw.eye.w <= 0.0 {
        return 0x7fu;
    }
    let eye = draw.eye.xyz - origin;
    let low = select(eye.y, min(draw.low.x - origin.y, eye.y), draw.low.y > 0.0);
    let m = SIDE_MARGIN;
    var mask = 0x4u;
    if eye.x > tiles[slot].planes[0] - m { mask |= 0x1u; }
    if eye.z > tiles[slot].planes[1] - m { mask |= 0x2u; }
    if eye.y > tiles[slot].planes[3] - m { mask |= 0x8u; }
    if eye.z < tiles[slot].planes[4] + m { mask |= 0x10u; }
    if eye.x < tiles[slot].planes[5] + m { mask |= 0x20u; }
    if low < tiles[slot].planes[6] + m { mask |= 0x40u; }
    return mask;
}

fn bucket_start(slot: u32, i: u32) -> u32 {
    if i == 0u {
        return 0u;
    }
    return tiles[slot].ends[i - 1u];
}

// How many of the tile's quads lie in the `mask`ed buckets.
fn seen_quads(slot: u32, mask: u32) -> u32 {
    var quads = 0u;
    for (var i = 0u; i < 7u; i++) {
        if (mask & (1u << i)) != 0u {
            quads += tiles[slot].ends[i] - bucket_start(slot, i);
        }
    }
    return quads;
}

// The `n`th quad of the tile's `mask`ed buckets, in mesh order: its index
// among all the tile's quads.
fn seen_quad(slot: u32, mask: u32, n: u32) -> u32 {
    var left = n;
    for (var i = 0u; i < 7u; i++) {
        if (mask & (1u << i)) == 0u {
            continue;
        }
        let start = bucket_start(slot, i);
        let size = tiles[slot].ends[i] - start;
        if left < size {
            return start + left;
        }
        left -= size;
    }
    return 0u;
}

@task @payload(payload) @workgroup_size(1, 1, 1)
fn ts_lod(@builtin(workgroup_id) wg: vec3<u32>) -> @builtin(mesh_task_size) vec3<u32> {
    if wg.x >= draw.params.y {
        return vec3<u32>(0u, 1u, 1u);
    }
    let entry = list[draw.params.x + wg.x];
    let slot = entry >> 8u;
    let first_meshlet = (entry & 0xffu) * MESHLETS_PER_TASK;
    let inst = instances[slot];
    if inst.w == 0.0 {
        return vec3<u32>(0u, 1u, 1u);
    }
    if tiles[slot].quads == 0u || !in_view(slot, inst.xyz) {
        return vec3<u32>(0u, 1u, 1u);
    }
    if culling() {
        let stamp = draw.params.w;
        if phase2() {
            // The box test saw it this frame, and phase 1 did not draw it.
            if tiles[slot].seen != stamp || tiles[slot].drawn == stamp {
                return vec3<u32>(0u, 1u, 1u);
            }
        } else {
            if !seen_lately(slot) {
                return vec3<u32>(0u, 1u, 1u);
            }
            tiles[slot].drawn = stamp;
        }
    }
    if first_meshlet == 0u {
        atomicAdd(&counts[select(0u, 1u, phase2())], 1u);
    }
    let mask = seen_buckets(slot, inst.xyz);
    let meshlets = (seen_quads(slot, mask) + QUADS_PER_MESHLET - 1u) / QUADS_PER_MESHLET;
    if first_meshlet >= meshlets {
        return vec3<u32>(0u, 1u, 1u);
    }
    payload = TaskPayload(slot, mask, first_meshlet);
    return vec3<u32>(min(meshlets - first_meshlet, MESHLETS_PER_TASK), 1u, 1u);
}

struct LoadedVertex {
    pos: vec3<f32>,
    uv: vec2<u32>,
    layer: u32,
    light: vec2<u32>,
    color: vec4<f32>,
}

// `TerrainVertex` number `index` of the page, as `vs_terrain` receives it.
fn load_vertex(index: u32) -> LoadedVertex {
    let b = index * 6u;
    let uv = verts[b + 3u];
    let layer_light = verts[b + 4u];
    let light = layer_light >> 16u;
    let c = verts[b + 5u];
    return LoadedVertex(
        vec3<f32>(bitcast<f32>(verts[b]), bitcast<f32>(verts[b + 1u]), bitcast<f32>(verts[b + 2u])),
        vec2<u32>(uv & 0xffffu, uv >> 16u),
        layer_light & 0xffffu,
        vec2<u32>(light & 0xffu, light >> 8u),
        vec4<f32>(f32(c & 0xffu), f32((c >> 8u) & 0xffu), f32((c >> 16u) & 0xffu), f32(c >> 24u)) / 255.0,
    );
}

fn meshlet_tint(meshlet: u32) -> vec3<f32> {
    let hue = f32((meshlet * 2654435761u) % 360u) / 360.0;
    let k = vec3<f32>(5.0, 3.0, 1.0);
    let p = abs(fract(vec3<f32>(hue) + k / 6.0) * 6.0 - 3.0);
    return mix(vec3<f32>(1.0), clamp(p - 1.0, vec3<f32>(0.0), vec3<f32>(1.0)), 0.75);
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

@mesh(out_mesh) @payload(payload) @workgroup_size(128, 1, 1)
fn ms_lod(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_index) lane: u32,
) {
    let slot = payload.slot;
    let mask = payload.mask;
    let inst = instances[slot];
    let first = (payload.first_meshlet + wg.x) * QUADS_PER_MESHLET;
    let live = min(QUADS_PER_MESHLET, seen_quads(slot, mask) - first);
    if lane == 0u {
        out_mesh.vertex_count = live * 4u;
        out_mesh.primitive_count = live * 2u;
    }
    // One vertex per lane (the vertex path's parallelism), the lanes of a
    // quad together. Every lane returns together at the end: the outputs are
    // copied out of `out_mesh` as each lane returns, so one returning early
    // would copy vertices the others have not written yet.
    let q = lane >> 2u;
    if q < live {
        let quad = seen_quad(slot, mask, first + q);
        let v = load_vertex(tiles[slot].first_vertex + quad * 4u + (lane & 3u));
        var out = terrain_vertex(v.pos, terrain_uv(v.uv, inst.w), v.layer, v.color, v.light,
            inst.xyz, inst.w);
        if (draw.params.z & 1u) != 0u {
            out.color = vec4(out.color.rgb * meshlet_tint(payload.first_meshlet + wg.x), out.color.a);
        }
        out_mesh.vertices[lane] = out;
    }
    // gfx.rs's `quad_indices`: corners 0, 1, 2 and 0, 2, 3.
    if lane < live * 2u {
        let v0 = (lane >> 1u) * 4u;
        out_mesh.primitives[lane].indices = select(
            vec3<u32>(v0, v0 + 1u, v0 + 2u),
            vec3<u32>(v0, v0 + 2u, v0 + 3u),
            (lane & 1u) != 0u,
        );
    }
    workgroupBarrier();
}

// The occlusion test of the far tiles, as cull.wgsl's of the sections: after
// phase 1, each listed tile's box is drawn against the depth with back faces
// culled and the depth test forced before the fragment shader, which marks
// the tile seen. A box failing everywhere would have every fragment of its
// tile fail the depth test against the final depth, which is only nearer.
// Phase 1's tiles are tested again one frame in `RETEST`, to notice when
// they go out of sight.

const BOX_CORNERS = array<u32, 36>(
    0u, 6u, 2u, 0u, 4u, 6u, 1u, 3u, 7u, 1u, 7u, 5u, 0u, 1u, 5u, 0u, 5u, 4u,
    2u, 7u, 3u, 2u, 6u, 7u, 0u, 3u, 1u, 0u, 2u, 3u, 4u, 5u, 7u, 4u, 7u, 6u,
);

struct TileBox {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) slot: u32,
}

fn box_corner(slot: u32, origin: vec3<f32>, c: u32) -> vec3<f32> {
    // Half a block out: the bounds are the quads' own.
    let lo = tiles[slot].bounds_min.xyz + origin - vec3(0.5);
    let hi = tiles[slot].bounds_max.xyz + origin + vec3(0.5);
    return vec3(
        select(lo.x, hi.x, (c & 1u) != 0u),
        select(lo.y, hi.y, (c & 2u) != 0u),
        select(lo.z, hi.z, (c & 4u) != 0u),
    );
}

@vertex
fn vs_tile_box(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> TileBox {
    var out: TileBox;
    out.position = vec4(0.0, 0.0, 0.0, 1.0);
    let entry = list[draw.params.x + i];
    let slot = entry >> 8u;
    out.slot = slot;
    let inst = instances[slot];
    let stamp = draw.params.w;
    // Once per tile (its first chunk), for one phase 1 did not draw, or one
    // of its whose turn it is.
    let retest = (slot + stamp) % RETEST == 0u;
    if (entry & 0xffu) != 0u || inst.w == 0.0 || (tiles[slot].drawn == stamp && !retest) {
        return out;
    }
    if v == 0u {
        atomicAdd(&counts[2], 1u);
        // Tested now: a tile not tested before counted as seen until here. Its
        // fragments set `seen` to the stamp where they pass (0 is no frame's).
        if tiles[slot].seen == FRESH {
            tiles[slot].seen = 0u;
        }
    }
    // The box's screen rectangle, from the corners in front of the camera.
    var lo = vec2(1e30);
    var hi = vec2(-1e30);
    var behind = false;
    // Against the depth as drawn: the frame's own projection (group 0), not the culling one.
    for (var c = 0u; c < 8u; c++) {
        let p = globals.view_proj * vec4(box_corner(slot, inst.xyz, c), 1.0);
        behind = behind || p.w <= 0.0;
        let q = p.xy / max(p.w, 1e-6);
        lo = min(lo, q);
        hi = max(hi, q);
    }
    let size = max(min(hi, vec2(1.0)) - max(lo, vec2(-1.0)), vec2(0.0));
    // A box the camera is in may cross the near plane, which clips its front
    // faces away, and one over a large part of the screen is nearly always in
    // sight and costly to test: both count as seen, through a small triangle
    // in the top left corner at the nearest depth.
    let near = all(box_corner(slot, inst.xyz, 0u) < vec3(0.0)) && all(box_corner(slot, inst.xyz, 7u) > vec3(0.0));
    if near || (!behind && size.x * size.y > BIG * 4.0) {
        var p = vec2(-1.0, 1.0);
        if v == 1u {
            p.y -= 0.05;
        } else if v == 2u {
            p.x += 0.05;
        }
        out.position = select(vec4(0.0, 0.0, 0.0, 1.0), vec4(p, 1.0, 1.0), v < 3u);
        return out;
    }
    out.position = globals.view_proj * vec4(box_corner(slot, inst.xyz, BOX_CORNERS[v]), 1.0);
    return out;
}

@fragment
@early_depth_test(force)
fn fs_tile_box(in: TileBox) -> @location(0) vec4<f32> {
    if tiles[in.slot].seen != draw.params.w {
        tiles[in.slot].seen = draw.params.w;
        atomicAdd(&counts[3], 1u);
    }
    return vec4(0.0);
}
