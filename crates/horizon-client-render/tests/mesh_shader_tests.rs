//! The GPU-driven far terrain (lod/pipeline.rs, lod_terrain_mesh.wgsl): its
//! module validates, its task shader's choice of quads repeats
//! `Sides::visible_from`, and on a mesh-shader GPU it draws a tile's quad
//! where the vertex path would, and nothing where that path draws nothing.
use horizon_client_render::{
    lod::{DrawParams, MeshCounts, MeshShaderPipeline, TileKey, TileRecord, mesh_shader_source},
    mesher::{Part, SectionMesh, Sides, TerrainVertex},
};
use naga::valid::{Capabilities, ValidationFlags, Validator};

const TERRAIN: &str = include_str!("../src/terrain.wgsl");

#[test]
fn far_terrain_mesh_module_validates() {
    let source = mesh_shader_source(TERRAIN);
    let module = naga::front::wgsl::parse_str(&source).expect("WGSL parses");
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("WGSL validates");
}

/// `ts_lod`'s `seen_buckets`: the buckets the eye sees, in `Sides`' order.
fn seen_mask(sides: &Sides, eye: Option<[f32; 3]>, low: Option<f32>) -> u32 {
    let Some(eye) = eye else {
        return 0x7f;
    };
    let planes = sides.planes();
    let low = low.map_or(eye[1], |l| l.min(eye[1]));
    let m = Sides::MARGIN;
    let mut mask = 0x4;
    if eye[0] > planes[0] - m {
        mask |= 0x1;
    }
    if eye[2] > planes[1] - m {
        mask |= 0x2;
    }
    if eye[1] > planes[3] - m {
        mask |= 0x8;
    }
    if eye[2] < planes[4] + m {
        mask |= 0x10;
    }
    if eye[0] < planes[5] + m {
        mask |= 0x20;
    }
    if low < planes[6] + m {
        mask |= 0x40;
    }
    mask
}

/// `ms_lod`'s `seen_quad` for every `n` of `seen_quads`: the quads a tile's
/// meshlets cover, in order.
fn seen_quads(sides: &Sides, mask: u32) -> Vec<u32> {
    let ends = sides.ends();
    let mut quads = Vec::new();
    for i in 0..7 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let start = if i == 0 { 0 } else { ends[i - 1] };
        quads.extend(start..ends[i]);
    }
    quads
}

/// A face across `axis` at `plane`, its other corners `a` and `b` in the
/// other two axes, wound so it faces the positive direction when `positive`.
fn face(axis: usize, plane: f32, positive: bool, a: f32, b: f32) -> [TerrainVertex; 4] {
    let (u, w) = ((axis + 1) % 3, (axis + 2) % 3);
    let corner = |du: f32, dw: f32| {
        let mut pos = [0.0; 3];
        pos[axis] = plane;
        pos[u] = a + du;
        pos[w] = b + dw;
        TerrainVertex::tiled(pos, [du, dw], 7, u32::MAX, 0x0f0f)
    };
    let mut corners = [
        corner(0.0, 0.0),
        corner(1.0, 0.0),
        corner(1.0, 1.0),
        corner(0.0, 1.0),
    ];
    if !positive {
        corners.reverse();
    }
    corners
}

/// A mesh with quads in every bucket, as the mesher sorts it.
fn every_bucket_mesh() -> SectionMesh {
    let mut solid = Vec::new();
    for axis in 0..3 {
        for positive in [true, false] {
            for i in 0..3 {
                let plane = [2.0, 5.0, 9.0][i] + if positive { 0.0 } else { 0.5 };
                solid.extend(face(axis, plane, positive, i as f32 * 2.0, 1.0));
            }
        }
    }
    // Sloped: the "other" bucket, drawn from everywhere.
    solid.extend([
        TerrainVertex::tiled([0.0, 0.0, 0.0], [0.0, 0.0], 7, u32::MAX, 0x0f0f),
        TerrainVertex::tiled([3.0, 1.0, 0.0], [1.0, 0.0], 7, u32::MAX, 0x0f0f),
        TerrainVertex::tiled([3.0, 2.0, 3.0], [1.0, 1.0], 7, u32::MAX, 0x0f0f),
        TerrainVertex::tiled([0.0, 1.0, 3.0], [0.0, 1.0], 7, u32::MAX, 0x0f0f),
    ]);
    let mut mesh = SectionMesh {
        solid,
        ..Default::default()
    };
    mesh.sort_solid();
    mesh
}

#[test]
fn task_shader_quads_are_the_runs_visible_from_sees() {
    let mesh = every_bucket_mesh();
    let sides = mesh.sides;
    assert_eq!(sides.quads(), 19);
    assert!(
        sides.ends().windows(2).all(|w| w[0] < w[1]),
        "every bucket has quads: {sides:?}"
    );
    // Eyes all around the mesh, and straight at its planes.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 11) as f32 / (1u64 << 53) as f32 * 24.0 - 6.0
    };
    let mut eyes: Vec<[f32; 3]> = (0..500).map(|_| [random(), random(), random()]).collect();
    for p in sides.planes() {
        for d in [-1.0, -Sides::MARGIN, 0.0, Sides::MARGIN, 1.0] {
            eyes.push([p + d; 3]);
        }
    }
    for eye in eyes {
        for low in [None, Some(eye[1] - 3.0), Some(eye[1] + 3.0)] {
            // As `facing_runs_from` calls it: the low eye is never above the eye.
            let mut expected = Vec::new();
            sides.visible_from(
                eye,
                low.map_or(eye[1], |l| l.min(eye[1])),
                Part::All,
                |first, count| expected.extend(first..first + count),
            );
            let got = seen_quads(&sides, seen_mask(&sides, Some(eye), low));
            assert_eq!(got, expected, "eye {eye:?}, low {low:?}");
        }
    }
    // Without an eye (`Renderer::sides_on` off), every quad.
    assert_eq!(seen_quads(&sides, seen_mask(&sides, None, None)).len(), 19);
}

static GPU_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Byte offset of `filtering` in terrain.wgsl's `Globals`.
const FILTERING_OFFSET: u64 = 2144;

fn mesh_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    if !adapter
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_MESH_SHADER)
    {
        return None;
    }
    // With the occlusion test's features where the adapter has them, as the renderer.
    let features = wgpu::Features::EXPERIMENTAL_MESH_SHADER
        | (adapter.features()
            & (wgpu::Features::VERTEX_WRITABLE_STORAGE | wgpu::Features::SHADER_EARLY_DEPTH_TEST));
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("mesh shader test device"),
        required_features: features,
        required_limits: adapter.limits(),
        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        ..Default::default()
    }))
    .ok()
}

/// terrain.wgsl's group 0 as the renderer lays it out: an identity projection
/// and no fog, white 1x1 sprites and lightmap.
fn terrain_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    use wgpu::{
        BindingType, BufferBindingType, SamplerBindingType, ShaderStages, TextureSampleType,
        TextureViewDimension,
    };
    // As the renderer's: the box test's vertex stage reads the globals too.
    let stages = ShaderStages::TASK | ShaderStages::MESH | ShaderStages::VERTEX_FRAGMENT;
    let texture = |binding, visibility, dimension| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Texture {
            sample_type: TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    };
    let sampler = |binding, visibility| wgpu::BindGroupLayoutEntry {
        binding,
        visibility,
        ty: BindingType::Sampler(SamplerBindingType::Filtering),
        count: None,
    };
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: stages,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        texture(1, ShaderStages::FRAGMENT, TextureViewDimension::D2Array),
        sampler(2, ShaderStages::FRAGMENT),
        texture(3, stages, TextureViewDimension::D2),
        sampler(4, stages),
    ];
    for binding in 5..12 {
        entries.push(texture(
            binding,
            ShaderStages::FRAGMENT,
            TextureViewDimension::D2Array,
        ));
    }
    entries.push(texture(
        12,
        ShaderStages::FRAGMENT,
        TextureViewDimension::D2,
    ));
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("test terrain bindings"),
        entries: &entries,
    });
    let mut globals = vec![0u8; 4096];
    let identity = glam::Mat4::IDENTITY.to_cols_array();
    globals[..64].copy_from_slice(bytemuck::cast_slice(&identity));
    // filtering.y: the sprite arrays' width in texels.
    let at = FILTERING_OFFSET as usize;
    globals[at..at + 16].copy_from_slice(bytemuck::cast_slice(&[0.0f32, 1.0, 0.0, 0.0]));
    let uniform = wgpu::util::DeviceExt::create_buffer_init(
        device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test globals"),
            contents: &globals,
            usage: wgpu::BufferUsages::UNIFORM,
        },
    );
    let white = |layers| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("white"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: layers,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for layer in 0..layers {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: 0,
                        y: 0,
                        z: layer,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &[255, 255, 255, 255],
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
            );
        }
        texture
    };
    let sprites = white(8);
    let lightmap = white(1);
    let thresholds = white(1);
    let array_view = |t: &wgpu::Texture| {
        t.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(TextureViewDimension::D2Array),
            ..Default::default()
        })
    };
    let sprite_views: Vec<_> = (0..8).map(|_| array_view(&sprites)).collect();
    let lightmap_view = lightmap.create_view(&Default::default());
    let thresholds_view = thresholds.create_view(&Default::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor::default());
    let mut group_entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(&sprite_views[0]),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::TextureView(&lightmap_view),
        },
        wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
    ];
    for binding in 5..12 {
        group_entries.push(wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(&sprite_views[binding as usize - 4]),
        });
    }
    group_entries.push(wgpu::BindGroupEntry {
        binding: 12,
        resource: wgpu::BindingResource::TextureView(&thresholds_view),
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("test terrain group"),
        layout: &layout,
        entries: &group_entries,
    });
    (layout, group)
}

struct Canvas {
    color: wgpu::Texture,
    depth: wgpu::TextureView,
    readback: wgpu::Buffer,
}

const SIDE: u32 = 64;

impl Canvas {
    fn new(device: &wgpu::Device) -> Self {
        let texture = |format, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some("mesh test target"),
                size: wgpu::Extent3d {
                    width: SIDE,
                    height: SIDE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        Self {
            color: texture(
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            ),
            depth: texture(
                wgpu::TextureFormat::Depth32Float,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            )
            .create_view(&Default::default()),
            readback: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mesh test readback"),
                size: u64::from(SIDE * SIDE * 4),
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            }),
        }
    }

    /// Draws the pipeline's pass 0 over a cleared target; the pixels, row by
    /// row from the top.
    fn draw(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        group: &wgpu::BindGroup,
        mesh: &MeshShaderPipeline,
    ) -> Vec<[u8; 4]> {
        let view = self.color.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("mesh test"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_bind_group(0, group, &[]);
            mesh.draw(&mut pass, 0, false);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.color,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &self.readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(SIDE * 4),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: SIDE,
                height: SIDE,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let slice = self.readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| tx.send(r).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let pixels: Vec<[u8; 4]> = slice
            .get_mapped_range()
            .expect("mapped")
            .as_chunks::<4>()
            .0
            .to_vec();
        self.readback.unmap();
        pixels
    }
}

/// How many of the target's pixels in each half (top, bottom) were drawn.
fn drawn_halves(pixels: &[[u8; 4]]) -> (usize, usize) {
    let drawn = |rows: std::ops::Range<u32>| {
        rows.flat_map(|y| (0..SIDE).map(move |x| (y * SIDE + x) as usize))
            .filter(|&i| pixels[i][3] != 0)
            .count()
    };
    (drawn(0..SIDE / 2), drawn(SIDE / 2..SIDE))
}

#[test]
#[ignore = "requires a Vulkan mesh-shader GPU"]
fn mesh_pipeline_draws_a_tiles_quad_as_the_vertex_path_would() {
    let _guard = GPU_LOCK.lock().unwrap();
    let Some((device, queue)) = mesh_device() else {
        eprintln!("no mesh-shader GPU: skipped");
        return;
    };
    let (terrain_layout, terrain_group) = terrain_group(&device, &queue);
    let mut mesh = MeshShaderPipeline::new(
        &device,
        &terrain_layout,
        wgpu::TextureFormat::Rgba8Unorm,
        [("SPRITE_ARRAYS", 1.0), ("SPRITE_SPLIT", 2048.0)],
        TERRAIN,
    )
    .expect("mesh pipeline");
    // One quad across the top half of an identity projection's clip space,
    // its vertices in an arena page as the mesher and uploader leave them.
    let mut tile = SectionMesh {
        solid: vec![
            TerrainVertex::tiled([-1.0, 0.0, 0.5], [0.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 0.0, 0.5], [1.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 1.0, 0.5], [1.0, 1.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([-1.0, 1.0, 0.5], [0.0, 1.0], 0, u32::MAX, 0x0f0f),
        ],
        ..Default::default()
    };
    tile.sort_solid();
    let page = wgpu::util::DeviceExt::create_buffer_init(
        &device,
        &wgpu::util::BufferInitDescriptor {
            label: Some("test arena page"),
            contents: bytemuck::cast_slice(&tile.solid),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::VERTEX,
        },
    );
    let [min, max] = tile.bounds.expect("bounds");
    let key = TileKey {
        x: 0,
        z: 0,
        level: 0,
    };
    mesh.set_tile(
        key,
        TileRecord {
            bounds: [min.to_array(), max.to_array()],
            page: 0,
            first_vertex: 0,
            sides: tile.sides,
        },
    );
    assert_eq!(mesh.tile_count(), 1);
    let canvas = Canvas::new(&device);
    let draw = |mesh: &mut MeshShaderPipeline,
                key: TileKey,
                origin: Option<[f32; 3]>,
                eye: Option<[f32; 3]>| {
        let visible = origin.map(|o| (key, [o[0], o[1], o[2], 1.0 / 32.0]));
        mesh.set_visible(visible.into_iter());
        mesh.begin_frame(
            &device,
            &mut |buffer, offset, bytes| queue.write_buffer(buffer, offset, bytes),
            |p| (p == 0).then_some(&page),
            0,
            &[DrawParams {
                view_proj: glam::Mat4::IDENTITY,
                eye,
                low: None,
                debug: false,
                phase: None,
            }],
        );
        // The uploads in a submission of their own, as the renderer does: RADV's
        // task shaders ran ahead of uploads in the draw's own command buffer.
        queue.submit([]);
        drawn_halves(&canvas.draw(&device, &queue, &terrain_group, mesh))
    };
    // Drawn whole, from no eye: the top half, white.
    let origin = Some([0.0, 0.0, 0.0]);
    let (top, bottom) = draw(&mut mesh, key, origin, None);
    assert_eq!((top, bottom), ((SIDE * SIDE / 2) as usize, 0));
    // The eye decides as `Sides::visible_from` does: the quad is seen from
    // one side of its plane and not the other.
    for z in [1.0, 0.0] {
        let eye = [0.0, 0.5, z];
        let mut expected = 0;
        tile.sides.visible(eye, |_, quads| expected += quads);
        let (top, _) = draw(&mut mesh, key, origin, Some(eye));
        assert_eq!(top > 0, expected > 0, "eye at z {z}");
    }
    // Out of the frustum: nothing. Not drawn this frame (no instance): nothing.
    assert_eq!(draw(&mut mesh, key, Some([5.0, 0.0, 0.0]), None), (0, 0));
    assert_eq!(draw(&mut mesh, key, None, None), (0, 0));
    // Removed: nothing, and its slot is reused.
    mesh.remove_tile(key);
    assert_eq!(mesh.tile_count(), 0);
    assert_eq!(draw(&mut mesh, key, origin, None), (0, 0));
    let other = TileKey {
        x: 1,
        z: 0,
        level: 0,
    };
    mesh.set_tile(
        other,
        TileRecord {
            bounds: [min.to_array(), max.to_array()],
            page: 0,
            first_vertex: 0,
            sides: tile.sides,
        },
    );
    assert_eq!((mesh.tile_count(), mesh.slot_count()), (1, 1));
    let (top, _) = draw(&mut mesh, other, origin, None);
    assert!(top > 0);
}

/// Frames submitted faster than their count readbacks come back (the loading
/// screen's, the title's) must not copy into a readback whose map is still
/// pending: wgpu fails the submission ("Buffer ... is still mapped"), which the
/// game treats as fatal.
#[test]
#[ignore = "requires a Vulkan mesh-shader GPU"]
fn counts_readbacks_skip_frames_until_their_maps_land() {
    let _guard = GPU_LOCK.lock().unwrap();
    let Some((device, queue)) = mesh_device() else {
        eprintln!("no mesh-shader GPU: skipped");
        return;
    };
    let (terrain_layout, _) = terrain_group(&device, &queue);
    let mut mesh = MeshShaderPipeline::new(
        &device,
        &terrain_layout,
        wgpu::TextureFormat::Rgba8Unorm,
        [("SPRITE_ARRAYS", 1.0), ("SPRITE_SPLIT", 2048.0)],
        TERRAIN,
    )
    .expect("mesh pipeline");
    let page = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test arena page"),
        size: 64,
        usage: wgpu::BufferUsages::STORAGE,
        mapped_at_creation: false,
    });
    let params = [DrawParams {
        view_proj: glam::Mat4::IDENTITY,
        eye: None,
        low: None,
        debug: false,
        phase: None,
    }];
    let frame = |mesh: &mut MeshShaderPipeline| {
        mesh.set_visible(std::iter::empty());
        mesh.begin_frame(
            &device,
            &mut |buffer, offset, bytes| queue.write_buffer(buffer, offset, bytes),
            |p| (p == 0).then_some(&page),
            0,
            &params,
        );
        let mut encoder = device.create_command_encoder(&Default::default());
        let read = mesh.end_frame(&mut encoder);
        queue.submit([encoder.finish()]);
        let copied = read.is_some();
        if let Some(read) = read {
            read();
        }
        copied
    };
    // The GPU busy for a while first, as it is with frames in flight, so the
    // copies behind it leave the maps pending (a submission's own poll would
    // land them otherwise). Then ten frames with no polling in between: once the
    // readbacks are all on their way, the frames skip the copy instead of
    // failing the submission.
    let busy = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("busy"),
        size: wgpu::Extent3d {
            width: 4096,
            height: 4096,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = busy.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    for _ in 0..2000 {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
    }
    queue.submit([encoder.finish()]);
    let copied = (0..10).map(|_| frame(&mut mesh)).filter(|&c| c).count();
    assert!(copied <= 3, "{copied} copies into 3 readbacks");
    // Once the maps land, the readbacks are read and reused.
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    assert!(frame(&mut mesh), "no readback freed after the maps landed");
    assert_eq!(mesh.counts(), MeshCounts::default());
}

#[test]
#[ignore = "requires a Vulkan mesh-shader GPU"]
fn tile_slots_are_recycled() {
    // The records alone: 200 rounds of 20 tiles coming and going keep the
    // slots to about the tiles alive at once.
    let _guard = GPU_LOCK.lock().unwrap();
    let Some((device, queue)) = mesh_device() else {
        eprintln!("no mesh-shader GPU: skipped");
        return;
    };
    let (terrain_layout, _) = terrain_group(&device, &queue);
    let mut mesh = MeshShaderPipeline::new(
        &device,
        &terrain_layout,
        wgpu::TextureFormat::Rgba8Unorm,
        [("SPRITE_ARRAYS", 1.0), ("SPRITE_SPLIT", 2048.0)],
        TERRAIN,
    )
    .expect("mesh pipeline");
    let tile = every_bucket_mesh();
    let record = TileRecord {
        bounds: [[0.0; 3], [16.0; 3]],
        page: 0,
        first_vertex: 0,
        sides: tile.sides,
    };
    for round in 0..200 {
        for i in 0..20 {
            mesh.set_tile(
                TileKey {
                    x: (round % 10) * 20 + i,
                    z: 0,
                    level: 0,
                },
                record,
            );
        }
        if round >= 2 {
            for i in 0..20 {
                mesh.remove_tile(TileKey {
                    x: ((round - 2) % 10) * 20 + i,
                    z: 0,
                    level: 0,
                });
            }
        }
    }
    assert_eq!(mesh.tile_count(), 40);
    assert!(mesh.slot_count() <= 60, "slots {}", mesh.slot_count());
}
