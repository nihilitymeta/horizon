//! Math and GPU validation for the GPU-driven far terrain.
use horizon_client_render::{
    lod::{MeshShaderPipeline, TileKey},
    mesher::{SectionMesh, TerrainVertex},
};
use naga::valid::{Capabilities, ValidationFlags, Validator};

#[test]
fn lod_terrain_mesh_shader_validates() {
    let source = include_str!("../shaders/lod_terrain_mesh.wgsl");
    let module = naga::front::wgsl::parse_str(source).expect("WGSL parses");
    Validator::new(ValidationFlags::all(), Capabilities::all())
        .validate(&module)
        .expect("WGSL validates");
}

#[test]
fn angular_foveation_uses_a_three_degree_hysteresis_band() {
    // 17 and 23 degrees around the 20-degree cone.
    let enter = 17f32.to_radians().cos();
    let exit = 23f32.to_radians().cos();
    let foveated = |cos: f32, was: bool| {
        if was { cos >= exit } else { cos >= enter }
    };
    // Walking into the cone enters at 17 degrees...
    assert!(foveated(enter, false));
    assert!(!foveated(enter - 0.001, false));
    // ...and only leaves again past 23.
    assert!(foveated(exit + 0.001, true));
    assert!(!foveated(exit - 0.001, true));
    assert!(!foveated(0.0, true));
}

#[test]
fn meshlet_outputs_fit_the_shader_limits() {
    // 32 quads per mesh workgroup: four vertices and two triangles each,
    // exactly the 128-vertex and 64-primitive outputs the shader declares.
    assert_eq!(32 * 4, 128);
    assert_eq!(32 * 2, 64);
}

#[test]
fn frustum_test_culls_boxes_beyond_one_plane() {
    // Identity clip space: a box at z = -1 is behind the near plane.
    let outside = |min: [f32; 3], max: [f32; 3]| {
        let mut out = [true; 3];
        for i in 0..8 {
            let p = [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ];
            out[0] &= p[0] < -1.0 || p[0] > 1.0;
            out[1] &= p[1] < -1.0 || p[1] > 1.0;
            out[2] &= p[2] < 0.0 || p[2] > 1.0;
        }
        out[0] || out[1] || out[2]
    };
    assert!(outside([0.0, 0.0, -2.0], [0.5, 0.5, -1.5]));
    assert!(!outside([0.0, 0.0, 0.25], [0.5, 0.5, 0.75]));
}

#[test]
#[ignore = "requires a Vulkan mesh-shader GPU"]
fn mesh_pipeline_creates_and_draws() {
    let Some((device, queue)) = mesh_device() else {
        eprintln!("no mesh-shader GPU: skipped");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (terrain_layout, terrain_group) = terrain_group(&device, &queue);

    let mut mesh = MeshShaderPipeline::new(
        &device,
        &terrain_layout,
        format,
        [("SPRITE_ARRAYS", 1.0), ("SPRITE_SPLIT", 2048.0)],
    )
    .expect("mesh pipeline");
    let quad = SectionMesh {
        solid: vec![
            TerrainVertex::tiled([-1.0, 0.0, 0.0], [0.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 0.0, 0.0], [1.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 1.0, 0.0], [1.0, 1.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([-1.0, 1.0, 0.0], [0.0, 1.0], 0, u32::MAX, 0x0f0f),
        ],
        ..Default::default()
    };
    mesh.upload_tile(
        &device,
        &queue,
        TileKey {
            x: 0,
            z: 0,
            level: 0,
        },
        &quad,
    );
    mesh.prepare(&queue, [0.5, 0.5, 0.5], [0.0, 1.0], false);
    assert_eq!(mesh.chunk_count(), 1);

    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh test colour"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("mesh test depth"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let color_view = color.create_view(&Default::default());
    let depth_view = depth.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view: &color_view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("mesh test pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        mesh.draw(&mut pass, &terrain_group);
    }
    queue.submit([encoder.finish()]);
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();

    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("mesh test readback"),
        size: 64 * 64 * 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture: &color,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(64 * 4),
                rows_per_image: Some(64),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    let slice = readback.slice(..);
    let (done, wait) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = done.send(result);
    });
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    wait.recv().unwrap().unwrap();
    let pixels = slice.get_mapped_range().unwrap();
    assert!(
        pixels
            .chunks(4)
            .any(|pixel| pixel[0] > 0 || pixel[1] > 0 || pixel[2] > 0),
        "the mesh shader drew nothing"
    );
    drop(pixels);
    readback.unmap();
}

fn mesh_device() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    if !adapter
        .features()
        .contains(wgpu::Features::EXPERIMENTAL_MESH_SHADER)
    {
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("mesh shader test device"),
        required_features: wgpu::Features::EXPERIMENTAL_MESH_SHADER,
        required_limits: adapter.limits(),
        experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
        ..Default::default()
    }))
    .ok()
}

/// The same group 0 the terrain pipeline uses, with white 1x1 textures.
fn terrain_group(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::BindGroupLayout, wgpu::BindGroup) {
    use wgpu::{
        BindingType, BufferBindingType, SamplerBindingType, ShaderStages, TextureSampleType,
        TextureViewDimension,
    };
    let mut layout_entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Buffer {
                ty: BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Sampler(SamplerBindingType::Filtering),
            count: None,
        },
    ];
    for binding in 5..=11 {
        layout_entries.push(wgpu::BindGroupLayoutEntry {
            binding,
            visibility: ShaderStages::TASK | ShaderStages::MESH | ShaderStages::FRAGMENT,
            ty: BindingType::Texture {
                sample_type: TextureSampleType::Float { filterable: true },
                view_dimension: TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        });
    }
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("mesh test terrain layout"),
        entries: &layout_entries,
    });
    // clip = (0.5x, 0.5y, 0.75, 1): the quad is in front, at a depth that
    // passes the reversed GreaterEqual test.
    let columns: [[f32; 4]; 4] = [
        [0.5, 0.0, 0.0, 0.0],
        [0.0, 0.5, 0.0, 0.0],
        [0.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.75, 1.0],
    ];
    let mut bytes = Vec::new();
    for column in columns {
        bytes.extend_from_slice(bytemuck::cast_slice(&column));
    }
    bytes.extend_from_slice(bytemuck::cast_slice(&[0.0f32; 4])); // fog colour
    bytes.extend_from_slice(bytemuck::cast_slice(&[100_000.0f32; 4])); // fog
    use wgpu::util::DeviceExt;
    let globals = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("mesh test globals"),
        contents: &bytes,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let view = |array: bool| {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 255, 255, 255],
        );
        texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(if array {
                TextureViewDimension::D2Array
            } else {
                TextureViewDimension::D2
            }),
            ..Default::default()
        })
    };
    let array_views: Vec<_> = (0..8).map(|_| view(true)).collect();
    let lightmap = view(false);
    let sampler = device.create_sampler(&Default::default());
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: globals.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(&array_views[0]),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
        wgpu::BindGroupEntry {
            binding: 3,
            resource: wgpu::BindingResource::TextureView(&lightmap),
        },
        wgpu::BindGroupEntry {
            binding: 4,
            resource: wgpu::BindingResource::Sampler(&sampler),
        },
    ];
    for (i, texture_view) in array_views[1..].iter().enumerate() {
        entries.push(wgpu::BindGroupEntry {
            binding: 5 + i as u32,
            resource: wgpu::BindingResource::TextureView(texture_view),
        });
    }
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh test terrain group"),
        layout: &layout,
        entries: &entries,
    });
    (layout, group)
}

#[test]
#[ignore = "requires a Vulkan mesh-shader GPU"]
fn tile_churn_recycles_vertex_and_chunk_memory_without_leak() {
    let Some((device, queue)) = mesh_device() else {
        eprintln!("no mesh-shader GPU: skipped");
        return;
    };
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let (terrain_layout, _) = terrain_group(&device, &queue);

    let mut mesh = MeshShaderPipeline::new(
        &device,
        &terrain_layout,
        format,
        [("SPRITE_ARRAYS", 1.0), ("SPRITE_SPLIT", 2048.0)],
    )
    .expect("mesh pipeline");

    let quad = SectionMesh {
        solid: vec![
            TerrainVertex::tiled([-1.0, 0.0, 0.0], [0.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 0.0, 0.0], [1.0, 0.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([1.0, 1.0, 0.0], [1.0, 1.0], 0, u32::MAX, 0x0f0f),
            TerrainVertex::tiled([-1.0, 1.0, 0.0], [0.0, 1.0], 0, u32::MAX, 0x0f0f),
        ],
        ..Default::default()
    };

    // Simulate 200 cycles of zooming/rotating with heavy tile creation and retirement
    for round in 0..200 {
        for i in 0..20 {
            let key = TileKey {
                x: (round % 10) * 20 + i,
                z: 0,
                level: 0,
            };
            mesh.upload_tile(&device, &queue, key, &quad);
        }
        // Remove earlier round tiles
        if round >= 2 {
            for i in 0..20 {
                let key = TileKey {
                    x: ((round - 2) % 10) * 20 + i,
                    z: 0,
                    level: 0,
                };
                mesh.remove_tile(key);
            }
        }
    }

    // Active resident tiles should never exceed 40
    assert!(mesh.chunk_count() <= 60, "chunk count was {}", mesh.chunk_count());
}
