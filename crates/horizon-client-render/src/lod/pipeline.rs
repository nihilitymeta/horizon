//! GPU-driven far terrain: the task/mesh pipeline over the solid quads of the
//! resident far tiles, where they already lie in the terrain arena, and the
//! buffers it reads. Nothing is copied: a tile's record names its arena page
//! and first vertex, its bounds and its side buckets, and the task shader
//! does per tile what `Renderer::render` does on the CPU for the vertex path
//! (the frustum test, the buckets that can face the eye, the draw list).
use crate::{fast_hash::FxHashMap, lod::TileKey, mesher::Sides};
use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use std::mem::size_of;

/// The task and mesh stages (`lod_terrain_mesh.wgsl`), appended to terrain.wgsl.
const MESH_STAGES: &str = include_str!("../../shaders/lod_terrain_mesh.wgsl");
/// `LodDraw`'s slot in the draws buffer: one per pass and page, at the dynamic
/// offset alignment.
const DRAW_STRIDE: u64 = 256;
/// `ts_lod`'s meshlets (of 32 quads) per task workgroup at most: a tile with
/// more is listed once per chunk of them, as written; `HORIZON_MESH_CAP`
/// varies it.
const MESHLETS_PER_TASK: u32 = 64;
/// A tile's `seen` before its first box test: phase 1 draws it.
const FRESH: u32 = u32::MAX;
/// Frames' counts on their way back from the GPU at once.
const COUNT_READBACKS: usize = 3;
const FREE: u8 = 0;
const PENDING: u8 = 1;
const MAPPED: u8 = 2;

/// A frame's counts, from the shaders (the `counts` binding): the tiles
/// phase 1 and phase 2 dispatched, the boxes tested and the boxes seen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub phase1: u32,
    pub phase2: u32,
    pub tested: u32,
    pub seen: u32,
}

/// The module: the mesh shader extension first, then terrain.wgsl whole (the
/// `terrain_vertex` and `fs_solid` the vertex path uses), then the stages.
pub fn shader_source(terrain: &str) -> String {
    format!("enable wgpu_mesh_shader;\n{terrain}\n{MESH_STAGES}")
}

/// `shader_source` with `quads` quads per meshlet (32 as written, or 64:
/// twice the lanes and outputs), for tuning (`HORIZON_MESH_QUADS`).
pub fn shader_source_with(terrain: &str, quads: u32) -> String {
    let source = shader_source(terrain);
    if quads == 32 {
        return source;
    }
    assert_eq!(quads, 64, "32 or 64 quads per meshlet");
    source
        .replace(
            "const QUADS_PER_MESHLET: u32 = 32u;",
            "const QUADS_PER_MESHLET: u32 = 64u;",
        )
        .replace(
            "@mesh(out_mesh) @payload(payload) @workgroup_size(128, 1, 1)",
            "@mesh(out_mesh) @payload(payload) @workgroup_size(256, 1, 1)",
        )
        .replace("array<TerrainOut, 128>", "array<TerrainOut, 256>")
        .replace("array<PrimitiveOut, 64>", "array<PrimitiveOut, 128>")
}

/// A tile's record on the GPU (`GpuTile` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct GpuTile {
    bounds_min: [f32; 4],
    bounds_max: [f32; 4],
    first_vertex: u32,
    quads: u32,
    /// Occlusion: the stamp of the last frame whose box test saw it
    /// (`FRESH` until tested), and of the last frame phase 1 drew it; the
    /// GPU writes both.
    seen: u32,
    drawn: u32,
    ends: [u32; 7],
    _pad1: u32,
    planes: [f32; 7],
    _pad2: f32,
}

/// One dispatch's uniforms (`LodDraw` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct LodDraw {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
    low: [f32; 4],
    params: [u32; 4],
}

/// Where a tile's solid quads are and how they are culled: what the vertex
/// path reads from its `SectionGpu` and `Tile` (gfx/lod.rs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRecord {
    /// Tile-relative box of its quads.
    pub bounds: [[f32; 3]; 2],
    pub page: u32,
    pub first_vertex: u32,
    pub sides: Sides,
}

/// Occlusion culling's phases (gfx/cull.rs, for the far tiles): phase 1
/// draws the tiles the box test saw lately, and is followed by the test
/// (`test_boxes`); phase 2 draws the tiles the test saw that phase 1 did
/// not draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    First,
    Second,
}

/// A pass's culling: what `Renderer::render` hands `facing_runs_from` and
/// `Bounds::visible`, camera-relative.
#[derive(Clone, Copy, Debug)]
pub struct DrawParams {
    pub view_proj: Mat4,
    /// `projection_center`, when the side buckets are culled.
    pub eye: Option<[f32; 3]>,
    /// The water mirror's low eye, a camera-relative height.
    pub low: Option<f32>,
    /// Colour each meshlet (F3 + M).
    pub debug: bool,
    /// The occlusion phase this draw is, when the frame culls.
    pub phase: Option<Phase>,
}

/// The mesh-shader far-terrain pipelines and their buffers.
pub struct MeshShaderPipeline {
    pipeline: wgpu::RenderPipeline,
    mirror_pipeline: wgpu::RenderPipeline,
    /// The tiles' box test (`test_boxes`), where the depth test can be forced early.
    box_pipeline: Option<wgpu::RenderPipeline>,
    /// The frame's stamp (`LodDraw::params.w`).
    stamp: u32,
    counts_buffer: wgpu::Buffer,
    /// Readbacks of `counts_buffer`, each `FREE`, `PENDING` (its map requested
    /// after the frame's submission) or `MAPPED` (read in `begin_frame`).
    count_readbacks: Vec<(wgpu::Buffer, std::sync::Arc<std::sync::atomic::AtomicU8>)>,
    count_next: usize,
    /// The last frame's counts read back.
    counts: Counts,
    layout: wgpu::BindGroupLayout,
    tiles: Vec<GpuTile>,
    /// Each slot's page; `u32::MAX` for a free slot.
    slot_pages: Vec<u32>,
    slots: FxHashMap<TileKey, u32>,
    free_slots: Vec<u32>,
    /// Slots whose record changed since the last `begin_frame`; every slot
    /// when `all_dirty`.
    dirty_slots: Vec<u32>,
    all_dirty: bool,
    instances: Vec<[f32; 4]>,
    /// This frame's visible tiles' slots by page (`page_ranges`), nearest
    /// first within a page, the shader's `list`.
    list: Vec<u32>,
    /// (page, first entry of `list`, entries).
    page_ranges: Vec<(u32, u32, u32)>,
    tiles_buffer: wgpu::Buffer,
    instances_buffer: wgpu::Buffer,
    list_buffer: wgpu::Buffer,
    draws_buffer: wgpu::Buffer,
    draws: Vec<u8>,
    /// Each page's bind group over the arena's page; remade when a buffer
    /// here is replaced (`rebind`) or a page is new.
    bind_groups: Vec<(u32, wgpu::BindGroup)>,
    rebind: bool,
    /// The arena's `generation` the bind groups were made at.
    page_generation: u64,
    /// Passes `begin_frame` was given this frame.
    passes: usize,
    /// `MESHLETS_PER_TASK` the module was built with.
    meshlets_per_task: u32,
    /// Each slot's quads: how many chunks its tile is listed as.
    slot_quads: Vec<u32>,
}

impl MeshShaderPipeline {
    pub fn new(
        device: &wgpu::Device,
        terrain_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        sprite_constants: [(&str, f64); 2],
        terrain_source: &str,
    ) -> Option<Self> {
        let quads = std::env::var("HORIZON_MESH_QUADS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|q| *q == 64)
            .unwrap_or(32);
        let limits = device.limits();
        log::info!(
            "Horizon: mesh shaders: {} quads per meshlet; limits: {} output vertices, {} primitives, \
             {} bytes of payload, {} invocations per workgroup",
            quads,
            limits.max_mesh_output_vertices,
            limits.max_mesh_output_primitives,
            limits.max_task_payload_size,
            limits.max_mesh_invocations_per_workgroup,
        );
        let mut source = shader_source_with(terrain_source, quads);
        if let Some(n) = std::env::var("HORIZON_MESH_CAP")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|n| (1..=255).contains(n))
        {
            source = source.replace(
                "const MESHLETS_PER_TASK: u32 = 64u;",
                &format!("const MESHLETS_PER_TASK: u32 = {n}u;"),
            );
            log::warn!("Horizon: mesh shader probe: {n} meshlets per task workgroup");
        }
        Self::from_source(device, terrain_layout, format, sprite_constants, &source)
    }

    /// `new` over a module of one's own (`shader_source`'s shape): tests and
    /// tools that vary the stages.
    pub fn from_source(
        device: &wgpu::Device,
        terrain_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        sprite_constants: [(&str, f64); 2],
        source: &str,
    ) -> Option<Self> {
        let limits = device.limits();
        // `fs_tile_box` forces the depth test before it runs, so a hidden box
        // marks nothing seen. Metal offers no early depth test through wgpu
        // (naga's MSL writer has none): there every box's fragments run and the
        // test sees every tile a nearer one hides, until it compares the depth
        // itself as `gfx/cull.rs`'s `DepthTest::InShader` does.
        let stripped;
        let source = if device
            .features()
            .contains(wgpu::Features::SHADER_EARLY_DEPTH_TEST)
        {
            source
        } else {
            stripped = source.replace("@early_depth_test(force)\n", "");
            log::info!(
                "Horizon: mesh shaders: no early depth test on this device: the tile box test \
                 shades every fragment"
            );
            &stripped
        };
        let big = source.contains("array<TerrainOut, 256>");
        let meshlets_per_task = source
            .split_once("const MESHLETS_PER_TASK: u32 = ")
            .and_then(|(_, rest)| rest.split_once('u'))
            .and_then(|(n, _)| n.parse().ok())
            .unwrap_or(MESHLETS_PER_TASK);
        if limits.max_mesh_output_vertices < if big { 256 } else { 128 }
            || limits.max_mesh_output_primitives < if big { 128 } else { 64 }
            || limits.max_task_payload_size < 4 * 65
        {
            log::warn!(
                "Horizon: mesh-shader limits too small ({} vertices, {} primitives, {} payload bytes)",
                limits.max_mesh_output_vertices,
                limits.max_mesh_output_primitives,
                limits.max_task_payload_size
            );
            return None;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Far terrain mesh shader"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        // The box test's vertex and fragment stages read them too, and the task
        // stage and the test's fragments write the tiles' `drawn` and `seen`.
        let stages = wgpu::ShaderStages::TASK
            | wgpu::ShaderStages::MESH
            | wgpu::ShaderStages::VERTEX
            | wgpu::ShaderStages::FRAGMENT;
        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: stages,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Far terrain mesh bindings"),
            entries: &[
                storage(0, false),
                storage(1, true),
                storage(2, true),
                storage(3, true),
                storage(5, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: stages,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(size_of::<LodDraw>() as u64),
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Far terrain mesh layout"),
            bind_group_layouts: &[Some(terrain_layout), Some(&layout)],
            ..Default::default()
        });
        let constants = &sprite_constants;
        let pipeline = |label: &str, fragment: &str| {
            device.create_mesh_pipeline(&wgpu::MeshPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                task: Some(wgpu::TaskState {
                    module: &module,
                    entry_point: Some("ts_lod"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        // `out_mesh` and the payload are written before they are read
                        // (`vertex_count` bounds the write-out), so no zero fill: Metal's
                        // would clear the workgroup's 14 KiB with one thread per meshlet.
                        zero_initialize_workgroup_memory: false,
                        ..Default::default()
                    },
                }),
                mesh: wgpu::MeshState {
                    module: &module,
                    entry_point: Some("ms_lod"),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        // `out_mesh` and the payload are written before they are read
                        // (`vertex_count` bounds the write-out), so no zero fill: Metal's
                        // would clear the workgroup's 14 KiB with one thread per meshlet.
                        zero_initialize_workgroup_memory: false,
                        ..Default::default()
                    },
                },
                // As "Terrain solid" (gfx.rs): back faces culled, reversed depth.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    compilation_options: wgpu::PipelineCompilationOptions {
                        constants,
                        ..Default::default()
                    },
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview: None,
                cache: None,
            })
        };
        let world = pipeline("Far terrain mesh", "fs_solid");
        let mirror = pipeline("Far terrain mesh mirror", "fs_solid_mirror");
        // The box test needs the depth test forced before its fragments (cull.rs).
        let box_pipeline = device
            .features()
            .contains(wgpu::Features::SHADER_EARLY_DEPTH_TEST)
            .then(|| {
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Far terrain tile boxes"),
                    layout: Some(&pipeline_layout),
                    vertex: wgpu::VertexState {
                        module: &module,
                        entry_point: Some("vs_tile_box"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState {
                        cull_mode: Some(wgpu::Face::Back),
                        ..Default::default()
                    },
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: wgpu::TextureFormat::Depth32Float,
                        depth_write_enabled: Some(false),
                        depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: Default::default(),
                    fragment: Some(wgpu::FragmentState {
                        module: &module,
                        entry_point: Some("fs_tile_box"),
                        compilation_options: wgpu::PipelineCompilationOptions {
                            constants,
                            ..Default::default()
                        },
                        targets: &[Some(wgpu::ColorTargetState {
                            format,
                            blend: None,
                            write_mask: wgpu::ColorWrites::empty(),
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
            });
        if let Some(error) = pollster::block_on(scope.pop()) {
            log::warn!("Horizon: mesh-shader pipeline unavailable: {error}");
            return None;
        }
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let counts_buffer = buffer(
            "Far terrain counts",
            16,
            storage | wgpu::BufferUsages::COPY_SRC,
        );
        let count_readbacks = (0..COUNT_READBACKS)
            .map(|_| {
                (
                    device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Far terrain counts readback"),
                        size: 16,
                        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    }),
                    std::sync::Arc::new(std::sync::atomic::AtomicU8::new(FREE)),
                )
            })
            .collect();
        Some(Self {
            pipeline: world,
            mirror_pipeline: mirror,
            box_pipeline,
            stamp: 0,
            counts_buffer,
            count_readbacks,
            count_next: 0,
            counts: Counts::default(),
            layout,
            tiles: Vec::new(),
            slot_pages: Vec::new(),
            slots: FxHashMap::default(),
            free_slots: Vec::new(),
            dirty_slots: Vec::new(),
            all_dirty: true,
            instances: Vec::new(),
            list: Vec::new(),
            page_ranges: Vec::new(),
            tiles_buffer: buffer(
                "Far terrain tiles",
                256 * size_of::<GpuTile>() as u64,
                storage,
            ),
            instances_buffer: buffer("Far terrain tile instances", 256 * 16, storage),
            list_buffer: buffer("Far terrain tile list", 256 * 4, storage),
            draws_buffer: buffer(
                "Far terrain draws",
                8 * DRAW_STRIDE,
                wgpu::BufferUsages::UNIFORM,
            ),
            draws: Vec::new(),
            bind_groups: Vec::new(),
            rebind: true,
            page_generation: 0,
            passes: 0,
            meshlets_per_task,
            slot_quads: Vec::new(),
        })
    }

    /// Records where the tile's solid quads are, replacing what it had.
    pub fn set_tile(&mut self, key: TileKey, record: TileRecord) {
        let quads = record.sides.quads();
        if quads == 0 {
            self.remove_tile(key);
            return;
        }
        let slot = match self.slots.get(&key) {
            Some(&slot) => slot,
            None => {
                let slot = self.free_slots.pop().unwrap_or_else(|| {
                    self.tiles.push(GpuTile::default());
                    self.slot_pages.push(u32::MAX);
                    self.slot_quads.push(0);
                    self.instances.push([0.0; 4]);
                    self.tiles.len() as u32 - 1
                });
                self.slots.insert(key, slot);
                slot
            }
        };
        let [min, max] = record.bounds;
        self.tiles[slot as usize] = GpuTile {
            bounds_min: [min[0], min[1], min[2], 0.0],
            bounds_max: [max[0], max[1], max[2], 0.0],
            first_vertex: record.first_vertex,
            quads,
            seen: FRESH,
            drawn: 0,
            ends: record.sides.ends(),
            planes: record.sides.planes(),
            ..Default::default()
        };
        self.slot_pages[slot as usize] = record.page;
        self.slot_quads[slot as usize] = quads;
        self.dirty_slots.push(slot);
    }

    pub fn remove_tile(&mut self, key: TileKey) {
        if let Some(slot) = self.slots.remove(&key) {
            self.tiles[slot as usize] = GpuTile::default();
            self.slot_pages[slot as usize] = u32::MAX;
            self.slot_quads[slot as usize] = 0;
            self.instances[slot as usize] = [0.0; 4];
            self.free_slots.push(slot);
            self.dirty_slots.push(slot);
        }
    }

    pub fn clear(&mut self) {
        self.tiles.clear();
        self.slot_pages.clear();
        self.slot_quads.clear();
        self.slots.clear();
        self.free_slots.clear();
        self.instances.clear();
        self.dirty_slots.clear();
        self.all_dirty = true;
    }

    /// Resident tiles with solid quads.
    pub fn tile_count(&self) -> u32 {
        self.slots.len() as u32
    }

    /// Whether the tile has a record.
    #[cfg(test)]
    pub fn has_tile(&self, key: TileKey) -> bool {
        self.slots.contains_key(&key)
    }

    /// Slots in use, the free ones between them included.
    pub fn slot_count(&self) -> u32 {
        self.tiles.len() as u32
    }

    /// Whether the far tiles can be occlusion culled here (`Phase`, `test_boxes`).
    pub fn culls(&self) -> bool {
        self.box_pipeline.is_some()
    }

    /// The last frame's counts that came back (`end_frame`).
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// This frame's tiles to draw, nearest first, as `Renderer::render` lists
    /// them for the vertex path (`visible_lods`): each with its instance
    /// (gfx/lod.rs's `instance`, its camera-relative origin and, in w, its UV
    /// step and fade code). The order holds within a page, so the near
    /// tiles' depth rejects the far ones' fragments as in the vertex path.
    pub fn set_visible(&mut self, visible: impl Iterator<Item = (TileKey, [f32; 4])>) {
        self.list.clear();
        self.page_ranges.clear();
        // (page, order, slot): a stable sort by page keeps the distance order.
        let mut by_page: Vec<(u32, u32, u32)> = Vec::new();
        for (key, instance) in visible {
            let Some(&slot) = self.slots.get(&key) else {
                continue;
            };
            self.instances[slot as usize] = instance;
            by_page.push((self.slot_pages[slot as usize], by_page.len() as u32, slot));
        }
        by_page.sort_unstable();
        // A tile once per chunk of `meshlets_per_task` meshlets its quads could fill
        // (the eye sees some buckets: the task shader drops the chunks past them).
        let quads_per_task = self.meshlets_per_task * 32;
        for (page, _, slot) in by_page {
            let chunks = self.slot_quads[slot as usize]
                .div_ceil(quads_per_task)
                .min(256);
            match self.page_ranges.last_mut() {
                Some((p, _, count)) if *p == page => *count += chunks,
                _ => self
                    .page_ranges
                    .push((page, self.list.len() as u32, chunks)),
            }
            for chunk in 0..chunks {
                self.list.push((slot << 8) | chunk);
            }
        }
    }

    /// Replaces `buffer` with one of `needed` bytes when it is smaller.
    fn grow(device: &wgpu::Device, buffer: &mut wgpu::Buffer, needed: u64) -> bool {
        if buffer.size() >= needed {
            return false;
        }
        let label = "Far terrain buffer";
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: needed.next_power_of_two(),
            usage: buffer.usage(),
            mapped_at_creation: false,
        });
        true
    }

    /// Uploads this frame's list and instances (`set_visible`), the passes'
    /// culling, and the records of tiles that changed, through `write` (the
    /// renderer's staging belt, or `Queue::write_buffer`); `pages` are the
    /// arena's live pages and `page_generation` its `generation`, so the
    /// bindings follow its buffers. Call once a frame before `draw`.
    pub fn begin_frame<'a>(
        &mut self,
        device: &wgpu::Device,
        write: &mut dyn FnMut(&wgpu::Buffer, u64, &[u8]),
        pages: impl Fn(u32) -> Option<&'a wgpu::Buffer>,
        page_generation: u64,
        passes: &[DrawParams],
    ) {
        if page_generation != self.page_generation {
            self.page_generation = page_generation;
            self.rebind = true;
        }
        self.stamp = self.stamp.wrapping_add(1).max(1);
        for (buffer, state) in &self.count_readbacks {
            if state.load(std::sync::atomic::Ordering::Acquire) != MAPPED {
                continue;
            }
            if let Ok(data) = buffer.slice(..).get_mapped_range() {
                let words: Vec<u32> = bytemuck::pod_collect_to_vec(&data);
                if let [phase1, phase2, tested, seen] = words[..] {
                    self.counts = Counts {
                        phase1,
                        phase2,
                        tested,
                        seen,
                    };
                }
            }
            buffer.unmap();
            state.store(FREE, std::sync::atomic::Ordering::Release);
        }
        write(&self.counts_buffer, 0, &[0; 16]);
        let tile_bytes = size_of::<GpuTile>() as u64;
        if Self::grow(
            device,
            &mut self.tiles_buffer,
            self.tiles.len().max(1) as u64 * tile_bytes,
        ) {
            self.all_dirty = true;
            self.rebind = true;
        }
        if self.all_dirty {
            if !self.tiles.is_empty() {
                write(&self.tiles_buffer, 0, bytemuck::cast_slice(&self.tiles));
            }
            self.all_dirty = false;
            self.dirty_slots.clear();
        } else if self.dirty_slots.len() > 8 {
            // Each write is a staging copy of its own: past a few, the whole table is cheaper.
            write(&self.tiles_buffer, 0, bytemuck::cast_slice(&self.tiles));
            self.dirty_slots.clear();
        } else if !self.dirty_slots.is_empty() {
            self.dirty_slots.sort_unstable();
            self.dirty_slots.dedup();
            for &slot in &self.dirty_slots {
                write(
                    &self.tiles_buffer,
                    u64::from(slot) * tile_bytes,
                    bytemuck::bytes_of(&self.tiles[slot as usize]),
                );
            }
            self.dirty_slots.clear();
        }
        self.rebind |= Self::grow(
            device,
            &mut self.instances_buffer,
            (self.instances.len().max(1) * 16) as u64,
        );
        self.rebind |= Self::grow(
            device,
            &mut self.list_buffer,
            (self.list.len().max(1) * 4) as u64,
        );
        if !self.instances.is_empty() {
            write(
                &self.instances_buffer,
                0,
                bytemuck::cast_slice(&self.instances),
            );
        }
        if !self.list.is_empty() {
            write(&self.list_buffer, 0, bytemuck::cast_slice(&self.list));
        }
        self.passes = passes.len();
        let draws = passes.len() * self.page_ranges.len();
        self.draws.clear();
        self.draws.resize(draws * DRAW_STRIDE as usize, 0);
        for (p, pass) in passes.iter().enumerate() {
            for (i, &(_, first, count)) in self.page_ranges.iter().enumerate() {
                let flags = u32::from(pass.debug)
                    | if pass.phase.is_some() { 2 } else { 0 }
                    | if pass.phase == Some(Phase::Second) {
                        4
                    } else {
                        0
                    };
                let draw = LodDraw {
                    view_proj: pass.view_proj.to_cols_array_2d(),
                    eye: pass.eye.map_or([0.0; 4], |e| [e[0], e[1], e[2], 1.0]),
                    low: pass.low.map_or([0.0; 4], |l| [l, 1.0, 0.0, 0.0]),
                    params: [first, count, flags, self.stamp],
                };
                let at = (p * self.page_ranges.len() + i) * DRAW_STRIDE as usize;
                self.draws[at..at + size_of::<LodDraw>()]
                    .copy_from_slice(bytemuck::bytes_of(&draw));
            }
        }
        if draws > 0 {
            self.rebind |= Self::grow(device, &mut self.draws_buffer, self.draws.len() as u64);
            write(&self.draws_buffer, 0, &self.draws);
        }
        if self.rebind {
            self.bind_groups.clear();
            self.rebind = false;
        }
        // A bind group for each page drawn this frame, kept for the next.
        for &(page, ..) in &self.page_ranges {
            if self.bind_groups.iter().any(|(p, _)| *p == page) {
                continue;
            }
            let Some(vertices) = pages(page) else {
                continue;
            };
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Far terrain mesh resources"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.tiles_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: vertices.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.instances_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: self.list_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 4,
                        resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: &self.draws_buffer,
                            offset: 0,
                            size: wgpu::BufferSize::new(size_of::<LodDraw>() as u64),
                        }),
                    },
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: self.counts_buffer.as_entire_binding(),
                    },
                ],
            });
            self.bind_groups.push((page, group));
        }
    }

    /// After the frame's passes: copies the counts out for reading back, into a
    /// free readback, which the returned closure maps once the frame is
    /// submitted (call it right after the submission). `None` when every
    /// readback is still on its way: its map requested but not done, or done and
    /// not yet read, since a copy into either fails the submission.
    pub fn end_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
    ) -> Option<impl FnOnce() + Send + 'static> {
        let i = self.count_next;
        let (buffer, state) = &self.count_readbacks[i];
        if state.load(std::sync::atomic::Ordering::Acquire) != FREE {
            return None;
        }
        encoder.copy_buffer_to_buffer(&self.counts_buffer, 0, buffer, 0, 16);
        self.count_next = (i + 1) % COUNT_READBACKS;
        state.store(PENDING, std::sync::atomic::Ordering::Release);
        let (buffer, state) = (buffer.clone(), state.clone());
        Some(move || {
            let done = state.clone();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| {
                    done.store(
                        if result.is_ok() { MAPPED } else { FREE },
                        std::sync::atomic::Ordering::Release,
                    );
                });
        })
    }

    /// After phase 1 (`Phase::First`, drawn), in a pass with the depth it
    /// left: the box test of the tiles phase 1 did not draw, and of a share
    /// of those it did. `pass_index` is phase 1's. Group 0 (terrain.wgsl's)
    /// must be bound.
    pub fn test_boxes(&self, pass: &mut wgpu::RenderPass<'_>, pass_index: usize) {
        let Some(pipeline) = &self.box_pipeline else {
            return;
        };
        if pass_index >= self.passes || self.page_ranges.is_empty() {
            return;
        }
        pass.set_pipeline(pipeline);
        for (i, &(page, _, count)) in self.page_ranges.iter().enumerate() {
            let Some((_, group)) = self.bind_groups.iter().find(|(p, _)| *p == page) else {
                continue;
            };
            let offset = ((pass_index * self.page_ranges.len() + i) as u64 * DRAW_STRIDE) as u32;
            pass.set_bind_group(1, group, &[offset]);
            pass.draw(0..36, 0..count);
        }
    }

    /// Draws every resident tile the pass sees: `pass_index` into
    /// `begin_frame`'s passes; `mirror` for the water mirror's fragment shader.
    /// Group 0 (terrain.wgsl's) must be bound.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, pass_index: usize, mirror: bool) {
        if pass_index >= self.passes || self.page_ranges.is_empty() {
            return;
        }
        pass.set_pipeline(if mirror {
            &self.mirror_pipeline
        } else {
            &self.pipeline
        });
        for (i, &(page, _, count)) in self.page_ranges.iter().enumerate() {
            let Some((_, group)) = self.bind_groups.iter().find(|(p, _)| *p == page) else {
                continue;
            };
            let offset = ((pass_index * self.page_ranges.len() + i) as u64 * DRAW_STRIDE) as u32;
            pass.set_bind_group(1, group, &[offset]);
            pass.draw_mesh_tasks(count, 1, 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpu_records_match_the_shader_layout() {
        // `GpuTile` and `LodDraw` in lod_terrain_mesh.wgsl, by std430 and std140.
        assert_eq!(size_of::<GpuTile>(), 112);
        assert_eq!(std::mem::offset_of!(GpuTile, first_vertex), 32);
        assert_eq!(std::mem::offset_of!(GpuTile, ends), 48);
        assert_eq!(std::mem::offset_of!(GpuTile, planes), 80);
        assert_eq!(size_of::<LodDraw>(), 112);
        assert!(size_of::<LodDraw>() as u64 <= DRAW_STRIDE);
    }
}
