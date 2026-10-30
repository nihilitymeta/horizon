//! On-screen hardware mesh shader indicator.
//!
//! Uses `VK_EXT_mesh_shader` (`@mesh` in WGSL) to generate geometric primitives directly
//! in GPU shared memory (LDS) without vertex or index buffers, providing an unmistakable
//! visual in-game demonstration that hardware mesh shaders are executing on the GPU.

const SHADER: &str = r#"
enable wgpu_mesh_shader;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
}

struct PrimitiveOut {
    @builtin(triangle_indices) indices: vec3<u32>,
}

struct MeshOut {
    @builtin(vertex_count) vertex_count: u32,
    @builtin(primitive_count) primitive_count: u32,
    @builtin(vertices) vertices: array<VertexOut, 32>,
    @builtin(primitives) primitives: array<PrimitiveOut, 16>,
}

var<workgroup> out_mesh: MeshOut;

@mesh(out_mesh) @workgroup_size(1, 1, 1)
fn ms_indicator() {
    out_mesh.vertex_count = 14u;
    out_mesh.primitive_count = 10u;

    // Glowing neon cyan / emerald badge in top right corner of the screen
    // Clip space coordinates: X in [0.72, 0.98], Y in [0.84, 0.96]
    let bg_color = vec4<f32>(0.02, 0.12, 0.10, 0.88);
    let border_color = vec4<f32>(0.0, 1.0, 0.75, 0.98);
    let accent_color = vec4<f32>(0.2, 0.95, 1.0, 1.0);

    // Outer badge hexagon / pill shape: Vertices 0..5
    out_mesh.vertices[0] = VertexOut(vec4<f32>(0.74, 0.96, 0.0, 1.0), bg_color);
    out_mesh.vertices[1] = VertexOut(vec4<f32>(0.96, 0.96, 0.0, 1.0), bg_color);
    out_mesh.vertices[2] = VertexOut(vec4<f32>(0.98, 0.90, 0.0, 1.0), border_color);
    out_mesh.vertices[3] = VertexOut(vec4<f32>(0.96, 0.84, 0.0, 1.0), bg_color);
    out_mesh.vertices[4] = VertexOut(vec4<f32>(0.74, 0.84, 0.0, 1.0), bg_color);
    out_mesh.vertices[5] = VertexOut(vec4<f32>(0.72, 0.90, 0.0, 1.0), border_color);

    // Primitives 0..3: badge background quad/hexagon
    out_mesh.primitives[0].indices = vec3<u32>(0u, 1u, 4u);
    out_mesh.primitives[1].indices = vec3<u32>(1u, 3u, 4u);
    out_mesh.primitives[2].indices = vec3<u32>(5u, 0u, 4u);
    out_mesh.primitives[3].indices = vec3<u32>(1u, 2u, 3u);

    // Inner wireframe / faceted mesh polygon showing "MESH" geometry: Vertices 6..13
    out_mesh.vertices[6] = VertexOut(vec4<f32>(0.76, 0.91, 0.0, 1.0), accent_color);
    out_mesh.vertices[7] = VertexOut(vec4<f32>(0.80, 0.93, 0.0, 1.0), border_color);
    out_mesh.vertices[8] = VertexOut(vec4<f32>(0.80, 0.87, 0.0, 1.0), border_color);
    out_mesh.vertices[9] = VertexOut(vec4<f32>(0.84, 0.91, 0.0, 1.0), accent_color);
    out_mesh.vertices[10] = VertexOut(vec4<f32>(0.88, 0.93, 0.0, 1.0), border_color);
    out_mesh.vertices[11] = VertexOut(vec4<f32>(0.88, 0.87, 0.0, 1.0), border_color);
    out_mesh.vertices[12] = VertexOut(vec4<f32>(0.92, 0.91, 0.0, 1.0), accent_color);
    out_mesh.vertices[13] = VertexOut(vec4<f32>(0.95, 0.90, 0.0, 1.0), border_color);

    // Primitives 4..9: faceted triangles forming the internal mesh lattice
    out_mesh.primitives[4].indices = vec3<u32>(6u, 7u, 8u);
    out_mesh.primitives[5].indices = vec3<u32>(7u, 9u, 8u);
    out_mesh.primitives[6].indices = vec3<u32>(9u, 10u, 11u);
    out_mesh.primitives[7].indices = vec3<u32>(10u, 12u, 11u);
    out_mesh.primitives[8].indices = vec3<u32>(11u, 12u, 13u);
    out_mesh.primitives[9].indices = vec3<u32>(10u, 13u, 12u);
}

@fragment
fn fs_indicator(in: VertexOut) -> @location(0) vec4<f32> {
    return in.color;
}
"#;

pub struct MeshIndicator {
    pipeline: wgpu::RenderPipeline,
}

impl MeshIndicator {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Option<Self> {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Mesh indicator WGSL"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let pipeline = device.create_mesh_pipeline(&wgpu::MeshPipelineDescriptor {
            label: Some("Mesh indicator pipeline"),
            layout: None,
            task: None,
            mesh: wgpu::MeshState {
                module: &shader,
                entry_point: Some("ms_indicator"),
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_indicator"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview: None,
            cache: None,
        });

        Some(Self { pipeline })
    }

    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
        let attachments = [Some(wgpu::RenderPassColorAttachment {
            view,
            resolve_target: None,
            depth_slice: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })];
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Mesh shader indicator pass"),
            color_attachments: &attachments,
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.draw_mesh_tasks(1, 1, 1);
    }
}
