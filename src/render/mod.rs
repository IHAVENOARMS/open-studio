//! GPU compositor shared by the editor preview and the exporter, so what you
//! see is what gets exported.

pub mod background;
pub mod cursor_image;

use crate::edit::{Background, EditConfig};
use crate::engine::{Camera, FrameState, Layout};
use egui_wgpu::wgpu;
use wgpu::util::DeviceExt;

pub const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SceneUniform {
    out_size: [f32; 2],
    aspect: f32,
    radius: f32,
    content: [f32; 4],
    cam: [f32; 4],
    prev_cam: [f32; 4],
    bg: [[f32; 4]; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteUniform {
    rect: [f32; 4],
    tint: [f32; 4],
}

struct VideoTexture {
    size: (u32, u32),
    texture: wgpu::Texture,
    scene_bind: wgpu::BindGroup,
    // (target level view, bind group sampling the level above)
    mip_chain: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    scene_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    mip_pipeline: wgpu::RenderPipeline,
    scene_layout: wgpu::BindGroupLayout,
    mip_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    scene_buf: wgpu::Buffer,
    sprite_buf: wgpu::Buffer,
    sprite_bind: wgpu::BindGroup,
    video: Option<VideoTexture>,
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl Compositor {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("open-studio shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });
        let scene_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scene"),
            entries: &[uniform_entry(0), texture_entry(1), sampler_entry(2)],
        });
        let mip_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mip"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });
        let pipeline = |layout: &wgpu::BindGroupLayout, vs: &str, fs: &str, blend: Option<wgpu::BlendState>| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(fs),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vs),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: if vs == "vs_sprite" {
                        wgpu::PrimitiveTopology::TriangleStrip
                    } else {
                        wgpu::PrimitiveTopology::TriangleList
                    },
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let scene_pipeline = pipeline(&scene_layout, "vs_full", "fs_scene", None);
        let sprite_pipeline =
            pipeline(&scene_layout, "vs_sprite", "fs_sprite", Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let mip_pipeline = pipeline(&mip_layout, "vs_full", "fs_mip", None);

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let scene_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scene uniform"),
            size: std::mem::size_of::<SceneUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let sprite_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite uniform"),
            size: std::mem::size_of::<SpriteUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (cw, ch) = cursor_image::SIZE;
        let cursor_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("cursor"),
                size: wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &cursor_image::rasterize(),
        );
        let sprite_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cursor"),
            layout: &scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: sprite_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&cursor_tex.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&sampler) },
            ],
        });

        Self {
            device,
            queue,
            scene_pipeline,
            sprite_pipeline,
            mip_pipeline,
            scene_layout,
            mip_layout,
            sampler,
            scene_buf,
            sprite_buf,
            sprite_bind,
            video: None,
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn ensure_video(&mut self, w: u32, h: u32) {
        if self.video.as_ref().is_some_and(|v| v.size == (w, h)) {
            return;
        }
        let mips = 32 - w.max(h).leading_zeros();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("video"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let level = |l: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: l,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        let mip_chain = (1..mips)
            .map(|l| {
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.mip_layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&level(l - 1)) },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    ],
                });
                (level(l), bind)
            })
            .collect();
        let scene_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.scene_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        });
        self.video = Some(VideoTexture { size: (w, h), texture, scene_bind, mip_chain });
    }

    /// Uploads an RGBA frame and rebuilds its mip chain (needed for clean
    /// downscaling of text when zoomed out).
    pub fn upload_frame(&mut self, rgba: &[u8], w: u32, h: u32) {
        self.ensure_video(w, h);
        let v = self.video.as_ref().unwrap();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &v.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let mut enc = self.device.create_command_encoder(&Default::default());
        for (view, bind) in &v.mip_chain {
            let mut pass = begin_pass(&mut enc, view);
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
    }

    pub fn has_frame(&self) -> bool {
        self.video.is_some()
    }

    /// Renders one composited frame into `target` (format [`FORMAT`]).
    pub fn render(
        &mut self,
        target: &wgpu::TextureView,
        out: (u32, u32),
        layout: &Layout,
        edit: &EditConfig,
        frame: &FrameState,
    ) -> wgpu::SubmissionIndex {
        let (ow, oh) = (out.0 as f32, out.1 as f32);
        let cam = frame.camera;
        let c = layout.content;
        let bg = match &edit.background {
            Background::Gradient(i) => {
                let p = &background::PRESETS[*i % background::PRESETS.len()];
                (1.0, p.corners.map(background::linear))
            }
            Background::Color(rgb) => {
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
                (0.0, [background::linear([to8(rgb[0]), to8(rgb[1]), to8(rgb[2])]); 4])
            }
        };
        let lod = match &self.video {
            Some(v) => ((v.size.1 as f64 / (c.h * oh as f64 * cam.scale)).log2().max(0.0)) as f32,
            None => 0.0,
        };
        let samples = if edit.motion_blur { blur_samples(layout, frame, oh as f64) } else { 1 };
        let u = SceneUniform {
            out_size: [ow, oh],
            aspect: layout.aspect as f32,
            radius: (edit.radius / 1080.0) as f32,
            content: [c.x as f32, c.y as f32, c.w as f32, c.h as f32],
            cam: [cam.cx as f32, cam.cy as f32, cam.scale as f32, edit.shadow as f32],
            prev_cam: [
                frame.prev_camera.cx as f32,
                frame.prev_camera.cy as f32,
                frame.prev_camera.scale as f32,
                samples as f32,
            ],
            bg: bg.1,
            params: [lod, bg.0, 0.0, 0.0],
        };
        self.queue.write_buffer(&self.scene_buf, 0, bytemuck::bytes_of(&u));

        let cursor = frame.cursor.filter(|c| c.opacity > 0.0);
        if let Some(cs) = cursor {
            let to_out = |x: f64, y: f64| {
                (((x - cam.cx) * cam.scale + layout.aspect * 0.5) * oh as f64, ((y - cam.cy) * cam.scale + 0.5) * oh as f64)
            };
            let (px, py) = to_out(cs.x, cs.y);
            let h = c.h * 0.035 * cs.scale * cam.scale * oh as f64;
            let w = h * cursor_image::SIZE.0 as f64 / cursor_image::SIZE.1 as f64;
            let x0 = px - w * cursor_image::HOTSPOT.0 as f64;
            let y0 = py - h * cursor_image::HOTSPOT.1 as f64;
            let ndc = |x: f64, y: f64| [(x / ow as f64 * 2.0 - 1.0) as f32, (1.0 - y / oh as f64 * 2.0) as f32];
            let a = ndc(x0, y0);
            let b = ndc(x0 + w, y0 + h);
            // Strip order (0,0),(1,0),(0,1),(1,1) with uv y down.
            let s = SpriteUniform { rect: [a[0], a[1], b[0], b[1]], tint: [1.0, 1.0, 1.0, cs.opacity as f32] };
            self.queue.write_buffer(&self.sprite_buf, 0, bytemuck::bytes_of(&s));
        }

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_pass(&mut enc, target);
            if let Some(v) = &self.video {
                pass.set_pipeline(&self.scene_pipeline);
                pass.set_bind_group(0, &v.scene_bind, &[]);
                pass.draw(0..3, 0..1);
                if cursor.is_some() {
                    pass.set_pipeline(&self.sprite_pipeline);
                    pass.set_bind_group(0, &self.sprite_bind, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
        }
        self.queue.submit([enc.finish()])
    }
}

fn begin_pass<'a>(enc: &'a mut wgpu::CommandEncoder, view: &'a wgpu::TextureView) -> wgpu::RenderPass<'a> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// How many sub-samples the motion blur needs: roughly one per 1.5 output
/// pixels of on-screen movement between the two camera states.
fn blur_samples(layout: &Layout, frame: &FrameState, out_h: f64) -> u32 {
    let (a, b) = (frame.prev_camera, frame.camera);
    if a == b {
        return 1;
    }
    let map = |cam: Camera, ux: f64, uy: f64| (cam.cx + (ux - layout.aspect * 0.5) / cam.scale, cam.cy + (uy - 0.5) / cam.scale);
    let mut max_px: f64 = 0.0;
    for (ux, uy) in [(0.0, 0.0), (layout.aspect, 0.0), (0.0, 1.0), (layout.aspect, 1.0), (layout.aspect * 0.5, 0.5)] {
        let (p, q) = (map(a, ux, uy), map(b, ux, uy));
        let d = ((p.0 - q.0).powi(2) + (p.1 - q.1).powi(2)).sqrt() * b.scale * out_h;
        max_px = max_px.max(d);
    }
    if max_px < 1.5 { 1 } else { ((max_px / 1.5).ceil() as u32).clamp(2, 16) }
}
