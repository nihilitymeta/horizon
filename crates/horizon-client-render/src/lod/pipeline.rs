//! GPU-driven far terrain: the task/mesh shader pipeline and the storage
//! buffers it reads. The CPU mesher's solid quads are uploaded unchanged, so
//! far terrain looks the same whether the mesh path or the vertex path draws
//! it; the task shader culls whole tiles and coarsens the periphery.
use crate::{fast_hash::FxHashMap, lod::TileKey, mesher::TerrainVertex};
use bytemuck::{Pod, Zeroable};
use std::mem::size_of;

const SHADER: &str = include_str!("../../shaders/lod_terrain_mesh.wgsl");
/// `TerrainVertex` as six `u32`s: position, packed UV, packed layer/light, colour.
const VERTEX_WORDS: u64 = 6;
const VERTEX_BYTE_SIZE: u64 = VERTEX_WORDS * 4;
const CHUNK_SIZE: u64 = size_of::<GpuChunkHeader>() as u64;
const INITIAL_VERTEX_CAPACITY: u64 = 16 * 1024 * 1024; // 16 MiB (~699k vertices)
const MAX_VERTEX_CAPACITY: u64 = 1024 * 1024 * 1024;   // 1 GiB (~44.7m vertices, well below 2GB limit)

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct GpuChunkHeader {
    /// World tile origin; w is the tile's fixed-point UV step (1/32).
    pub origin: [f32; 4],
    pub bounds_min: [f32; 4],
    pub bounds_max: [f32; 4],
    pub vertex_offset: u32,
    pub quad_count: u32,
    /// Bit 0: inside the foveation cone, persisted for the hysteresis band.
    pub flags: u32,
    pub _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct LodGlobals {
    pub camera: [f32; 4],
    /// xz forward look, z cos(enter), w cos(exit).
    pub forward: [f32; 4],
    /// x debug colouring, y resident chunk count.
    pub params: [u32; 4],
}

struct Tile {
    chunk: usize,
    vertex_offset: u32,
    vertex_count: u32,
}

/// The mesh-shader terrain pipeline and its shared buffers.
pub struct MeshShaderPipeline {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    globals: wgpu::Buffer,
    chunks: wgpu::Buffer,
    vertices: wgpu::Buffer,
    chunk_capacity: usize,
    vertex_capacity: u64,
    chunk_data: Vec<GpuChunkHeader>,
    free_chunks: Vec<usize>,
    free_vertex_ranges: Vec<(u32, u32)>,
    vertex_high_water: u32,
    tile_vertices: FxHashMap<TileKey, Vec<TerrainVertex>>,
    tiles: FxHashMap<TileKey, Tile>,
    chunk_count: u32,
    chunks_dirty: bool,
}

impl MeshShaderPipeline {
    pub fn new(
        device: &wgpu::Device,
        terrain_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        sprite_constants: [(&str, f64); 2],
    ) -> Option<Self> {
        let limits = device.limits();
        if limits.max_mesh_output_vertices < 128
            || limits.max_mesh_output_primitives < 64
            || limits.max_task_payload_size < 16
        {
            log::warn!(
                "Horizon: mesh-shader limits too small ({} vertices, {} primitives, {} payload)",
                limits.max_mesh_output_vertices,
                limits.max_mesh_output_primitives,
                limits.max_task_payload_size
            );
            return None;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("LOD terrain mesh shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("LOD terrain mesh bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::TASK
                        | wgpu::ShaderStages::MESH
                        | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::TASK
                        | wgpu::ShaderStages::MESH
                        | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::TASK
                        | wgpu::ShaderStages::MESH
                        | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("LOD terrain mesh layout"),
            bind_group_layouts: &[Some(terrain_layout), Some(&bind_group_layout)],
            ..Default::default()
        });
        let constants = &sprite_constants;
        let pipeline = device.create_mesh_pipeline(&wgpu::MeshPipelineDescriptor {
            label: Some("LOD terrain mesh pipeline"),
            layout: Some(&layout),
            task: Some(wgpu::TaskState {
                module: &module,
                entry_point: Some("ts_lod"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
            }),
            mesh: wgpu::MeshState {
                module: &module,
                entry_point: Some("ms_lod"),
                compilation_options: wgpu::PipelineCompilationOptions {
                    constants,
                    ..Default::default()
                },
            },
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
                entry_point: Some("fs_lod"),
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
        });

        if let Some(error) = pollster::block_on(scope.pop()) {
            log::warn!("Horizon: mesh-shader pipeline unavailable: {error}");
            return None;
        }
        let chunk_capacity = 256;
        let vertex_capacity = INITIAL_VERTEX_CAPACITY;
        let chunks = Self::chunk_buffer(device, chunk_capacity);
        let vertices = Self::vertex_buffer(device, vertex_capacity);
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LOD terrain mesh globals"),
            size: size_of::<LodGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = Self::bind_group(device, &bind_group_layout, &chunks, &vertices, &globals);
        Some(Self {
            pipeline,
            bind_group_layout,
            bind_group,
            globals,
            chunks,
            vertices,
            chunk_capacity,
            vertex_capacity,
            chunk_data: Vec::new(),
            free_chunks: Vec::new(),
            free_vertex_ranges: Vec::new(),
            vertex_high_water: 0,
            tile_vertices: FxHashMap::default(),
            tiles: FxHashMap::default(),
            chunk_count: 0,
            chunks_dirty: false,
        })
    }

    fn chunk_buffer(device: &wgpu::Device, chunks: usize) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LOD terrain chunks"),
            size: CHUNK_SIZE * chunks as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn vertex_buffer(device: &wgpu::Device, bytes: u64) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("LOD terrain vertices"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        chunks: &wgpu::Buffer,
        vertices: &wgpu::Buffer,
        globals: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("LOD terrain mesh resources"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: chunks.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: vertices.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: globals.as_entire_binding(),
                },
            ],
        })
    }

    fn ensure_chunk_capacity(&mut self, device: &wgpu::Device) {
        if self.chunk_data.len() > self.chunk_capacity {
            self.chunk_capacity = self.chunk_data.len().next_power_of_two();
            self.chunks = Self::chunk_buffer(device, self.chunk_capacity);
            self.bind_group = Self::bind_group(
                device,
                &self.bind_group_layout,
                &self.chunks,
                &self.vertices,
                &self.globals,
            );
            self.chunks_dirty = true;
        }
    }

    fn ensure_vertex_capacity(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, needed: u32) {
        let capacity_in_verts = (self.vertex_capacity / VERTEX_BYTE_SIZE) as u32;
        if self.vertex_high_water + needed <= capacity_in_verts {
            return;
        }

        // Try defragmenting if free ranges hold significant space
        let free_space: u32 = self.free_vertex_ranges.iter().map(|r| r.1).sum();
        let active_space = self.vertex_high_water.saturating_sub(free_space);
        if active_space + needed <= capacity_in_verts && free_space > (capacity_in_verts / 8) {
            self.defragment(queue);
            if self.vertex_high_water + needed <= capacity_in_verts {
                return;
            }
        }

        // Need to grow buffer
        let needed_bytes = (u64::from(self.vertex_high_water + needed) * VERTEX_BYTE_SIZE)
            .min(MAX_VERTEX_CAPACITY);
        let new_capacity = (self.vertex_capacity * 2).max(needed_bytes).min(MAX_VERTEX_CAPACITY);
        if new_capacity > self.vertex_capacity {
            self.vertex_capacity = new_capacity;
            self.vertices = Self::vertex_buffer(device, self.vertex_capacity);
            self.bind_group = Self::bind_group(
                device,
                &self.bind_group_layout,
                &self.chunks,
                &self.vertices,
                &self.globals,
            );
            self.defragment(queue);
        } else {
            self.defragment(queue);
        }
    }

    fn defragment(&mut self, queue: &wgpu::Queue) {
        let mut offset = 0u32;
        for (key, tile) in &mut self.tiles {
            if let Some(verts) = self.tile_vertices.get(key) {
                tile.vertex_offset = offset;
                if let Some(chunk) = self.chunk_data.get_mut(tile.chunk) {
                    chunk.vertex_offset = offset;
                }
                queue.write_buffer(
                    &self.vertices,
                    u64::from(offset) * VERTEX_BYTE_SIZE,
                    bytemuck::cast_slice(verts),
                );
                offset += verts.len() as u32;
            }
        }
        self.vertex_high_water = offset;
        self.free_vertex_ranges.clear();
        self.chunks_dirty = true;
    }

    fn alloc_vertices(&mut self, count: u32) -> Option<u32> {
        let mut best_idx = None;
        let mut best_waste = u32::MAX;
        for (i, &(_off, len)) in self.free_vertex_ranges.iter().enumerate() {
            if len >= count && (len - count) < best_waste {
                best_waste = len - count;
                best_idx = Some(i);
            }
        }
        if let Some(i) = best_idx {
            let (off, len) = self.free_vertex_ranges[i];
            if len == count {
                self.free_vertex_ranges.swap_remove(i);
            } else {
                self.free_vertex_ranges[i] = (off + count, len - count);
            }
            return Some(off);
        }

        let capacity_in_verts = (self.vertex_capacity / VERTEX_BYTE_SIZE) as u32;
        if self.vertex_high_water + count <= capacity_in_verts {
            let off = self.vertex_high_water;
            self.vertex_high_water += count;
            return Some(off);
        }

        None
    }

    fn free_vertices(&mut self, offset: u32, count: u32) {
        if count == 0 {
            return;
        }
        if offset + count == self.vertex_high_water {
            self.vertex_high_water = offset;
            return;
        }
        self.free_vertex_ranges.push((offset, count));
        if self.free_vertex_ranges.len() > 32 {
            self.merge_free_ranges();
        }
    }

    fn merge_free_ranges(&mut self) {
        if self.free_vertex_ranges.is_empty() {
            return;
        }
        self.free_vertex_ranges.sort_unstable_by_key(|r| r.0);
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(self.free_vertex_ranges.len());
        for (off, count) in self.free_vertex_ranges.drain(..) {
            if let Some(last) = merged.last_mut()
                && last.0 + last.1 == off
            {
                last.1 += count;
                continue;
            }
            merged.push((off, count));
        }
        if let Some(last) = merged.last()
            && last.0 + last.1 == self.vertex_high_water
        {
            self.vertex_high_water = last.0;
            merged.pop();
        }
        self.free_vertex_ranges = merged;
    }

    /// Appends or updates a tile's solid quads.
    pub fn upload_tile(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: TileKey,
        mesh: &crate::mesher::SectionMesh,
    ) {
        if mesh.solid.is_empty() {
            self.remove_tile(key);
            return;
        }

        self.remove_tile(key);

        let quads = (mesh.solid.len() / 4) as u32;
        let count = mesh.solid.len() as u32;

        self.ensure_vertex_capacity(device, queue, count);
        let Some(offset) = self.alloc_vertices(count) else {
            return;
        };

        let origin = key.origin();
        let mut min = [f32::MAX; 3];
        let mut max = [f32::MIN; 3];
        for vertex in &mesh.solid {
            for axis in 0..3 {
                min[axis] = min[axis].min(vertex.pos[axis] + origin[axis] as f32);
                max[axis] = max[axis].max(vertex.pos[axis] + origin[axis] as f32);
            }
        }
        let chunk = GpuChunkHeader {
            origin: [
                origin[0] as f32,
                origin[1] as f32,
                origin[2] as f32,
                1.0 / crate::mesher::TILED_UV_STEPS,
            ],
            bounds_min: [min[0], min[1], min[2], 0.0],
            bounds_max: [max[0], max[1], max[2], 0.0],
            vertex_offset: offset,
            quad_count: quads,
            flags: 0,
            _pad: 0,
        };

        let chunk_idx = if let Some(free_idx) = self.free_chunks.pop() {
            free_idx
        } else {
            self.chunk_data.push(chunk);
            self.chunk_data.len() - 1
        };

        if chunk_idx >= self.chunk_data.len() {
            self.chunk_data.resize(chunk_idx + 1, GpuChunkHeader::zeroed());
        }
        self.chunk_data[chunk_idx] = chunk;
        self.tiles.insert(
            key,
            Tile {
                chunk: chunk_idx,
                vertex_offset: offset,
                vertex_count: count,
            },
        );
        self.tile_vertices.insert(key, mesh.solid.clone());
        self.chunk_count = self.chunk_count.max(chunk_idx as u32 + 1);

        self.ensure_chunk_capacity(device);

        queue.write_buffer(
            &self.vertices,
            u64::from(offset) * VERTEX_BYTE_SIZE,
            bytemuck::cast_slice(&mesh.solid),
        );
        queue.write_buffer(
            &self.chunks,
            chunk_idx as u64 * CHUNK_SIZE,
            bytemuck::bytes_of(&chunk),
        );
        self.chunks_dirty = true;
    }

    pub fn remove_tile(&mut self, key: TileKey) {
        if let Some(tile) = self.tiles.remove(&key) {
            self.tile_vertices.remove(&key);
            if let Some(chunk) = self.chunk_data.get_mut(tile.chunk) {
                chunk.quad_count = 0;
                chunk.flags = 0;
                self.chunks_dirty = true;
            }
            self.free_chunks.push(tile.chunk);
            self.free_vertices(tile.vertex_offset, tile.vertex_count);
            while self.chunk_data.last().is_some_and(|c| c.quad_count == 0) {
                let dead_idx = self.chunk_data.len() - 1;
                self.chunk_data.pop();
                self.free_chunks.retain(|&idx| idx != dead_idx);
            }
            self.chunk_count = self.chunk_data.len() as u32;
        }
    }

    pub fn clear(&mut self) {
        self.chunk_data.clear();
        self.free_chunks.clear();
        self.free_vertex_ranges.clear();
        self.vertex_high_water = 0;
        self.tile_vertices.clear();
        self.tiles.clear();
        self.chunk_count = 0;
        self.chunks_dirty = true;
    }

    pub fn chunk_count(&self) -> u32 {
        self.chunk_count
    }

    /// Writes this frame's camera, look and debug state; uploads changed chunks.
    pub fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        camera: [f32; 3],
        forward: [f32; 2],
        debug: bool,
    ) {
        let globals = LodGlobals {
            camera: [camera[0], camera[1], camera[2], 0.0],
            // 17 and 23 degrees: enter and exit the 20-degree cone.
            forward: [forward[0], forward[1], 0.956_304_8, 0.920_504_85],
            params: [u32::from(debug), self.chunk_count, 0, 0],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
        if self.chunks_dirty {
            if !self.chunk_data.is_empty() {
                queue.write_buffer(&self.chunks, 0, bytemuck::cast_slice(&self.chunk_data));
            }
            self.chunks_dirty = false;
        }
    }

    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, terrain_group: &wgpu::BindGroup) {
        if self.chunk_count == 0 {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, terrain_group, &[]);
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.draw_mesh_tasks(self.chunk_count, 1, 1);
    }
}
