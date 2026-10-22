//! GPU compositor shared by the editor preview and the exporter, so what you
//! see is what gets exported.

pub mod background;
pub mod cursor_image;
pub mod captions;
pub mod keycaps;
pub mod looks;
pub mod raster;

use crate::edit::{Background, EditConfig};
use crate::engine::{CursorShape, FrameState, Layout};
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
    next_cam: [f32; 4],
    blur: [f32; 4],
    /// x = number of masks.
    mask_info: [f32; 4],
    /// Per mask: video uv x0, y0, x1, y1.
    mask_rects: [[f32; 4]; 8],
    /// Per mask: kind (0 blur, 1 highlight), amount, unused, unused.
    mask_params: [[f32; 4]; 8],
    bg: [[f32; 4]; 4],
    params: [f32; 4],
    /// Card rect (screen plus inset), canvas units.
    card: [f32; 4],
    /// Inset colour (linear) and opacity; w = 0 when there is no inset.
    inset: [f32; 4],
    /// Shadow offset x, y, blur radius, inner screen radius.
    shadow: [f32; 4],
    /// Click effect: x, y, progress, kind (0 none, 1 circle, 2 ripple).
    click: [f32; 4],
    /// Click effect base radius, unused x3.
    click2: [f32; 4],
    /// Loupe centre (video uv), radius (video heights), magnification.
    loupe: [f32; 4],
    /// Loupe grow amount (0 = off), video aspect.
    loupe2: [f32; 4],
    /// Screen crop: x, y, w, h as fractions of the recording.
    crop: [f32; 4],
    /// Screen card placement: offset x, y, scale.
    screen_xf: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CameraUniform {
    a: [f32; 4],
    b: [f32; 4],
    rect: [f32; 4],
    params: [f32; 4],
    uv: [f32; 4],
    lod: [f32; 4],
    /// Look: [exposure, contrast, saturation, temperature], [tint, fade, split, mono].
    grade0: [f32; 4],
    grade1: [f32; 4],
    /// Look strength, LUT size (0 = none), unused x2.
    grade2: [f32; 4],
    /// Cutout amount (0..1), matte available (0/1), shadow strength, unused.
    cut: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct SpriteUniform {
    /// Corners in NDC, strip order: top-left, top-right (a); bottom-left,
    /// bottom-right (b).
    a: [f32; 4],
    b: [f32; 4],
    tint: [f32; 4],
}

/// A cursor image on the GPU, with its size and hotspot in cursor units
/// (see [`cursor_image::PX_PER_UNIT`]).
struct Sprite {
    bind: wgpu::BindGroup,
    buf: wgpu::Buffer,
    size: (f32, f32),
    hotspot: (f32, f32),
}

/// Cursor units per nominal (24 px theme) cursor pixel.
const UNITS_PER_THEME_PX: f32 = 0.9;

struct VideoTexture {
    size: (u32, u32),
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    // (target level view, bind group sampling the level above)
    mip_chain: Vec<(wgpu::TextureView, wgpu::BindGroup)>,
}

pub struct Compositor {
    device: wgpu::Device,
    queue: wgpu::Queue,
    scene_pipeline: wgpu::RenderPipeline,
    sprite_pipeline: wgpu::RenderPipeline,
    camera_pipeline: wgpu::RenderPipeline,
    mip_pipeline: wgpu::RenderPipeline,
    scene_layout: wgpu::BindGroupLayout,
    mip_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    scene_buf: wgpu::Buffer,
    /// Cursor sprites by shape key, created on first use.
    sprites: std::collections::HashMap<String, Option<Sprite>>,
    /// Keycap overlay image for the shortcut on screen.
    keycaps: Option<(String, Sprite)>,
    /// Caption image for the line on screen.
    caption: Option<(String, Sprite)>,
    video: Option<VideoTexture>,
    /// Camera picture (webcam or phone camera track).
    camera: Option<VideoTexture>,
    camera_buf: wgpu::Buffer,
    camera_bind: Option<wgpu::BindGroup>,
    /// Camera pass bindings: the scene ones plus the matte (binding 4).
    camera_layout: wgpu::BindGroupLayout,
    /// Background-removal matte for the camera, when there is one.
    matte: Option<VideoTexture>,
    matte_placeholder: wgpu::TextureView,
    /// Camera LUT (path, view, size); a 1x1 placeholder when none.
    lut: (Option<String>, wgpu::TextureView, u32),
    /// Background image (1x1 placeholder when unused) and what it was loaded from.
    bg_view: wgpu::TextureView,
    bg_key: Option<String>,
    bg_aspect: f32,
    bg_height: u32,
    scene_bind: Option<wgpu::BindGroup>,
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
            entries: &[uniform_entry(0), texture_entry(1), sampler_entry(2), texture_entry(3)],
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
                    topology: if vs == "vs_sprite" || vs == "vs_camera" {
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
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("camera"),
            entries: &[uniform_entry(0), texture_entry(1), sampler_entry(2), texture_entry(3), texture_entry(4)],
        });
        let camera_pipeline =
            pipeline(&camera_layout, "vs_camera", "fs_camera", Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING));
        let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

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

        let placeholder = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("bg placeholder"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0, 0, 0, 255],
        );

        let placeholder_lut = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("lut placeholder"),
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[0, 0, 0, 255],
        );
        Self {
            device,
            queue,
            scene_pipeline,
            sprite_pipeline,
            camera_pipeline,
            mip_pipeline,
            scene_layout,
            mip_layout,
            sampler,
            scene_buf,
            sprites: Default::default(),
            keycaps: None,
            caption: None,
            video: None,
            camera: None,
            camera_buf,
            camera_bind: None,
            lut: (None, placeholder_lut.create_view(&Default::default()), 0),
            camera_layout,
            matte: None,
            matte_placeholder: placeholder_lut.create_view(&Default::default()),
            bg_view: placeholder.create_view(&Default::default()),
            bg_key: None,
            bg_aspect: 1.0,
            bg_height: 1,
            scene_bind: None,
        }
    }

    fn make_sprite(&self, rgba_premul: &[u8], w: u32, h: u32, size: (f32, f32), hotspot: (f32, f32)) -> Sprite {
        let tex = self.device.create_texture_with_data(
            &self.queue,
            &wgpu::TextureDescriptor {
                label: Some("cursor"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba_premul,
        );
        let view = tex.create_view(&Default::default());
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sprite uniform"),
            size: std::mem::size_of::<SpriteUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cursor"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&view) },
            ],
        });
        Sprite { bind, buf, size, hotspot }
    }

    /// Loads the sprite for a cursor shape: our vector drawing when we have
    /// one, otherwise the theme's largest image, or the captured image.
    fn load_sprite(&self, shape: &CursorShape) -> Option<Sprite> {
        match shape {
            CursorShape::Named { name, theme } => {
                if let Some(dir) = theme {
                    if let Some(img) = crate::xcursor::load_from_dir(std::path::Path::new(dir), name, 96) {
                        let k = UNITS_PER_THEME_PX * 24.0 / img.size as f32;
                        return Some(self.make_sprite(
                            &img.rgba,
                            img.width,
                            img.height,
                            (img.width as f32 * k, img.height as f32 * k),
                            (img.hotspot.0 as f32 * k, img.hotspot.1 as f32 * k),
                        ));
                    }
                }
                if let Some(v) = cursor_image::Vector::for_name(name) {
                    let n = cursor_image::CANVAS;
                    let u = cursor_image::PX_PER_UNIT;
                    let hot = v.hotspot();
                    return Some(self.make_sprite(&cursor_image::rasterize(v), n, n, (n as f32 / u, n as f32 / u), (hot.0 / u, hot.1 / u)));
                }
                let img = crate::xcursor::load_named(name, 96)?;
                let k = UNITS_PER_THEME_PX * 24.0 / img.size as f32;
                Some(self.make_sprite(
                    &img.rgba,
                    img.width,
                    img.height,
                    (img.width as f32 * k, img.height as f32 * k),
                    (img.hotspot.0 as f32 * k, img.hotspot.1 as f32 * k),
                ))
            }
            CursorShape::Custom { path, hotspot, scale } => {
                let mut img = image::open(path).ok()?.to_rgba8();
                for px in img.pixels_mut() {
                    let a = px[3] as u32;
                    for c in 0..3 {
                        px[c] = (px[c] as u32 * a / 255) as u8;
                    }
                }
                let (w, h) = img.dimensions();
                let k = UNITS_PER_THEME_PX / *scale as f32 * 24.0 / crate::xcursor::base_size() as f32;
                Some(self.make_sprite(&img, w, h, (w as f32 * k, h as f32 * k), (hotspot.0 as f32 * k, hotspot.1 as f32 * k)))
            }
        }
    }

    fn rebuild_scene_bind(&mut self) {
        let Some(v) = &self.video else { return };
        self.scene_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scene"),
            layout: &self.scene_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.scene_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&v.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.bg_view) },
            ],
        }));
    }

    /// Loads (once) the image behind a wallpaper/image background. Returns
    /// false when it isn't available, so the caller can fall back.
    fn ensure_background_image(&mut self, key: &str) -> bool {
        if self.bg_key.as_deref() == Some(key) {
            return true;
        }
        let path = crate::wallpapers::resolve(key);
        let Ok(img) = image::open(&path) else { return false };
        let img = img.to_rgba8();
        let (w, h) = img.dimensions();
        let mips = 32 - w.max(h).leading_zeros();
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("background"),
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
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &img,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let chain = self.mip_chain(&texture, mips);
        let mut enc = self.device.create_command_encoder(&Default::default());
        for (view, bind) in &chain {
            let mut pass = begin_pass(&mut enc, view);
            pass.set_pipeline(&self.mip_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([enc.finish()]);
        self.bg_view = texture.create_view(&Default::default());
        self.bg_key = Some(key.to_owned());
        self.bg_aspect = w as f32 / h as f32;
        self.bg_height = h;
        self.rebuild_scene_bind();
        true
    }

    /// Per-level render targets and bind groups sampling the level above.
    fn mip_chain(&self, texture: &wgpu::Texture, mips: u32) -> Vec<(wgpu::TextureView, wgpu::BindGroup)> {
        let level = |l: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                base_mip_level: l,
                mip_level_count: Some(1),
                ..Default::default()
            })
        };
        (1..mips)
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
            .collect()
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Uploads a camera frame (RGBA) and rebuilds its mip chain.
    pub fn upload_camera_frame(&mut self, rgba: &[u8], w: u32, h: u32) {
        if !self.camera.as_ref().is_some_and(|v| v.size == (w, h)) {
            self.camera = Some(self.video_texture(w, h));
            self.rebuild_camera_bind();
        }
        self.upload_into(self.camera.as_ref().unwrap(), rgba);
    }

    /// Uploads a background-removal matte frame (greyscale as RGBA).
    pub fn upload_matte_frame(&mut self, rgba: &[u8], w: u32, h: u32) {
        if !self.matte.as_ref().is_some_and(|v| v.size == (w, h)) {
            self.matte = Some(self.video_texture(w, h));
            self.rebuild_camera_bind();
        }
        self.upload_into(self.matte.as_ref().unwrap(), rgba);
    }

    fn rebuild_camera_bind(&mut self) {
        let Some(tex) = &self.camera else { return };
        let matte = self.matte.as_ref().map_or(&self.matte_placeholder, |m| &m.view);
        self.camera_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("camera"),
            layout: &self.camera_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.camera_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&tex.view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&self.lut.1) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(matte) },
            ],
        }));
    }

    /// Loads the camera LUT when the setting changes. Returns its size (0 = none).
    fn ensure_lut(&mut self, path: Option<&String>) -> u32 {
        if self.lut.0.as_ref() == path {
            return self.lut.2;
        }
        let loaded = path.and_then(|p| looks::load_cube(std::path::Path::new(p)).ok());
        let (view, size) = match loaded {
            Some(lut) => {
                let n = lut.size;
                let tex = self.device.create_texture_with_data(
                    &self.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("camera lut"),
                        size: wgpu::Extent3d { width: n * n, height: n, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &lut.rgba,
                );
                (tex.create_view(&Default::default()), n)
            }
            None => {
                let tex = self.device.create_texture_with_data(
                    &self.queue,
                    &wgpu::TextureDescriptor {
                        label: Some("lut placeholder"),
                        size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: wgpu::TextureFormat::Rgba8Unorm,
                        usage: wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    },
                    wgpu::util::TextureDataOrder::LayerMajor,
                    &[0, 0, 0, 255],
                );
                (tex.create_view(&Default::default()), 0)
            }
        };
        self.lut = (path.cloned(), view, size);
        self.rebuild_camera_bind();
        size
    }

    fn video_texture(&self, w: u32, h: u32) -> VideoTexture {
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
        let mip_chain = self.mip_chain(&texture, mips);
        let view = texture.create_view(&Default::default());
        VideoTexture { size: (w, h), texture, view, mip_chain }
    }

    fn upload_into(&self, v: &VideoTexture, rgba: &[u8]) {
        let (w, h) = v.size;
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
        let mip_chain = self.mip_chain(&texture, mips);
        let view = texture.create_view(&Default::default());
        self.video = Some(VideoTexture { size: (w, h), texture, view, mip_chain });
        self.rebuild_scene_bind();
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
        let card = layout.card;
        let lut_size = if frame.camera_view.is_some() { self.ensure_lut(edit.camera.lut.as_ref()) } else { 0 };
        let gradient = |i: usize| {
            let p = &background::PRESETS[i % background::PRESETS.len()];
            (1.0, p.corners.map(background::linear))
        };
        let image_key = match &edit.background {
            Background::Wallpaper(k) | Background::Image(k) => Some(k.clone()),
            _ => None,
        };
        let bg = match &edit.background {
            Background::Gradient(i) => gradient(*i),
            Background::Color(rgb) => {
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
                (0.0, [background::linear([to8(rgb[0]), to8(rgb[1]), to8(rgb[2])]); 4])
            }
            Background::Wallpaper(_) | Background::Image(_) => {
                if self.ensure_background_image(image_key.as_deref().unwrap()) {
                    (2.0, [[self.bg_aspect, 0.0, 0.0, 0.0]; 4])
                } else {
                    gradient(0)
                }
            }
        };
        // Background mip level: natural minification plus the blur amount.
        let bg_lod = {
            let natural = (self.bg_height as f64 / (oh as f64 * cam.scale)).log2().max(0.0);
            (natural + edit.background_blur * 7.0) as f32
        };
        // Texels per output pixel; below ~1 we are magnifying.
        let density = match &self.video {
            Some(v) => v.size.1 as f64 * layout.crop[3] / (c.h * oh as f64 * cam.scale),
            None => 1.0,
        };
        let lod = density.log2().max(0.0) as f32;
        let bicubic = if density < 0.9 { 1.0 } else { 0.0 };
        let samples = if edit.motion_blur { blur_samples(layout, frame, edit, oh as f64) } else { 1 };
        let u = SceneUniform {
            out_size: [ow, oh],
            aspect: layout.aspect as f32,
            radius: (edit.radius / 1080.0) as f32,
            content: [c.x as f32, c.y as f32, c.w as f32, c.h as f32],
            cam: [cam.cx as f32, cam.cy as f32, cam.scale as f32, edit.shadow as f32],
            prev_cam: [frame.blur_from.cx as f32, frame.blur_from.cy as f32, frame.blur_from.scale as f32, samples as f32],
            next_cam: [frame.blur_to.cx as f32, frame.blur_to.cy as f32, frame.blur_to.scale as f32, 0.0],
            blur: [edit.zoom_blur as f32, edit.pan_blur as f32, 0.0, 0.0],
            mask_info: [frame.masks.len() as f32, 0.0, 0.0, 0.0],
            mask_rects: std::array::from_fn(|i| {
                frame.masks.get(i).map_or([0.0; 4], |m| {
                    let r = m.rect;
                    [r[0] as f32, r[1] as f32, (r[0] + r[2]) as f32, (r[1] + r[3]) as f32]
                })
            }),
            mask_params: std::array::from_fn(|i| {
                frame.masks.get(i).map_or([0.0; 4], |m| {
                    let kind = match m.kind {
                        crate::edit::MaskKind::Blur => 0.0,
                        crate::edit::MaskKind::Highlight => 1.0,
                    };
                    [kind, m.amount as f32, 0.0, 0.0]
                })
            }),
            bg: bg.1,
            params: [lod, bg.0, bicubic, bg_lod],
            card: [card.x as f32, card.y as f32, card.w as f32, card.h as f32],
            inset: {
                let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0) as u8;
                let i = &edit.inset;
                let rgb = background::linear(i.color.map(to8));
                let on = i.enabled && (c.w < card.w || c.h < card.h);
                [rgb[0], rgb[1], rgb[2], if on { i.opacity as f32 } else { 0.0 }]
            },
            shadow: {
                let radius = (edit.radius / 1080.0) as f32;
                let inner = if edit.inset.enabled {
                    let i = ((card.h - c.h) * 0.5) as f32;
                    (radius - i).max(radius * 0.4)
                } else {
                    radius
                };
                match edit.shadow_style {
                    crate::edit::ShadowStyle::Classic => [0.0, 0.012, 0.035, inner],
                    crate::edit::ShadowStyle::Directional => {
                        // Angle 0 = light from the top, so the shadow falls down.
                        let a = edit.shadow_angle.to_radians();
                        let d = edit.shadow_distance * 0.06;
                        [(-a.sin() * d) as f32, (a.cos() * d) as f32, (0.008 + edit.shadow_softness * 0.07) as f32, inner]
                    }
                }
            },
            click: frame.click.map_or([0.0; 4], |k| {
                let kind = match k.kind {
                    crate::edit::ClickVisual::None => 0.0,
                    crate::edit::ClickVisual::Circle => 1.0,
                    crate::edit::ClickVisual::Ripple => 2.0,
                };
                [k.x as f32, k.y as f32, k.progress as f32, kind]
            }),
            click2: [(c.h * 0.035 * edit.cursor.size * frame.screen_xf[2]) as f32, 0.0, 0.0, 0.0],
            loupe: frame.loupe.map_or([0.0; 4], |l| [l.x as f32, l.y as f32, l.radius as f32, l.magnification as f32]),
            loupe2: [frame.loupe.map_or(0.0, |l| l.amount.max(0.0) as f32), (layout.video_w / layout.video_h) as f32, 0.0, 0.0],
            crop: layout.crop.map(|v| v as f32),
            screen_xf: [frame.screen_xf[0] as f32, frame.screen_xf[1] as f32, frame.screen_xf[2].max(1e-3) as f32, 0.0],
        };
        self.queue.write_buffer(&self.scene_buf, 0, bytemuck::bytes_of(&u));

        let cursor = frame.cursor.as_ref().filter(|c| c.opacity > 0.0);
        let mut sprite_key = None;
        if let Some(cs) = cursor {
            let key = format!("{:?}", cs.shape);
            if !self.sprites.contains_key(&key) {
                let mut sprite = self.load_sprite(&cs.shape);
                if sprite.is_none() {
                    let theme = match &cs.shape {
                        CursorShape::Named { theme, .. } => theme.clone(),
                        _ => None,
                    };
                    sprite = self.load_sprite(&CursorShape::Named { name: "default".into(), theme });
                }
                self.sprites.insert(key.clone(), sprite);
            }
            sprite_key = Some(key);
        }
        let sprite = sprite_key.as_ref().and_then(|k| self.sprites.get(k)).and_then(|s| s.as_ref());
        if let (Some(cs), Some(sp)) = (cursor, sprite) {
            let to_out = |x: f64, y: f64| {
                (((x - cam.cx) * cam.scale + layout.aspect * 0.5) * oh as f64, ((y - cam.cy) * cam.scale + 0.5) * oh as f64)
            };
            let (px, py) = to_out(cs.x, cs.y);
            // Output pixels per cursor unit; an arrow is ~3.5% of the video height at size 1.
            let k = c.h * 0.035 * cs.scale * cam.scale * oh as f64 / 176.0 * cursor_image::PX_PER_UNIT as f64;
            let (w, h) = (sp.size.0 as f64 * k, sp.size.1 as f64 * k);
            // Corners relative to the hotspot, rotated about it.
            let (hx, hy) = (sp.hotspot.0 as f64 * k, sp.hotspot.1 as f64 * k);
            let (sn, cn) = cs.rotation.sin_cos();
            let corner = |x: f64, y: f64| {
                let (dx, dy) = (x - hx, y - hy);
                ndc(ow, oh, px + dx * cn - dy * sn, py + dx * sn + dy * cn)
            };
            let s = quad([corner(0.0, 0.0), corner(w, 0.0), corner(0.0, h), corner(w, h)], cs.opacity);
            self.queue.write_buffer(&sp.buf, 0, bytemuck::bytes_of(&s));
        }

        // Camera picture, in canvas space (not affected by the screen zoom).
        let camera = frame.camera_view.filter(|_| self.camera.is_some());
        if let (Some(cv), Some(tex)) = (camera, &self.camera) {
            let k = oh as f64; // output pixels per canvas unit
            let r = cv.rect;
            let (x, y, w, h) = (r.x * k, r.y * k, r.w * k, r.h * k);
            // Room around the picture for its shadow.
            let m = w.max(h) * 0.12;
            let corners = [ndc(ow, oh, x - m, y - m), ndc(ow, oh, x + w + m, y - m), ndc(ow, oh, x - m, y + h + m), ndc(ow, oh, x + w + m, y + h + m)];
            // Cover-crop the kept part of the source (Camera crop, moved by
            // face tracking) to the picture's aspect.
            let [cx0, cy0, cw, ch] = cv.crop;
            let (tw, th) = (tex.size.0 as f64 * cw, tex.size.1 as f64 * ch);
            let (src, dst) = (tw / th, w / h.max(1e-6));
            let local = if src > dst {
                let f = dst / src;
                [(1.0 - f) / 2.0, 0.0, (1.0 + f) / 2.0, 1.0]
            } else {
                let f = src / dst;
                [0.0, (1.0 - f) / 2.0, 1.0, (1.0 + f) / 2.0]
            };
            let uv = [cx0 + local[0] * cw, cy0 + local[1] * ch, cx0 + local[2] * cw, cy0 + local[3] * ch];
            let texels = th * (local[3] - local[1]) / h.max(1.0);
            let st = &edit.camera;
            let [grade0, grade1] = st.look.grade().uniform();
            let strength = if st.look == looks::Look::None && lut_size == 0 { 0.0 } else { st.look_strength as f32 };
            let u = CameraUniform {
                a: [corners[0][0], corners[0][1], corners[1][0], corners[1][1]],
                b: [corners[2][0], corners[2][1], corners[3][0], corners[3][1]],
                rect: [x as f32, y as f32, w as f32, h as f32],
                params: [(cv.radius * k) as f32, cv.opacity as f32, cv.shadow as f32, if cv.mirror { 1.0 } else { 0.0 }],
                uv: uv.map(|v| v as f32),
                lod: [texels.log2().max(0.0) as f32, 0.0, 0.0, 0.0],
                grade0,
                grade1,
                grade2: [strength, lut_size as f32, 0.0, 0.0],
                cut: [cv.cutout as f32, if self.matte.is_some() { 1.0 } else { 0.0 }, cv.shadow as f32, 0.0],
            };
            self.queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(&u));
        }

        // Captions: centred near the bottom (or top) of the frame.
        let caption = frame.caption.as_ref().filter(|c| c.opacity > 0.0 && !c.words.is_empty());
        let mut caption_top = oh as f64 * 0.94;
        if let Some(cf) = caption {
            let st = &edit.captions;
            let font_px = (oh as f64 * 0.042 * st.size).round().max(10.0) as u32;
            let id = format!("{:?}|{}|{}|{font_px}|{:?}|{:?}|{:?}", cf.words, cf.rtl, cf.spoken, st.font, st.color, st.display);
            if self.caption.as_ref().map(|(k, _)| k) != Some(&id) {
                let img = captions::render(&cf.words, cf.rtl, cf.spoken, st, font_px);
                let sp = self.make_sprite(&img.rgba, img.width, img.height, (img.width as f32, img.height as f32), (0.0, 0.0));
                self.caption = Some((id, sp));
            }
            let (_, sp) = self.caption.as_ref().unwrap();
            let (w, h) = (sp.size.0 as f64, sp.size.1 as f64);
            let margin = oh as f64 * 0.06;
            let y0 = match st.position {
                crate::edit::CaptionPosition::Bottom => oh as f64 - margin - h,
                crate::edit::CaptionPosition::Top => margin,
            } + cf.slide * h;
            if st.position == crate::edit::CaptionPosition::Bottom {
                caption_top = y0 - cf.slide * h;
            }
            let x0 = (ow as f64 - w) * 0.5;
            let s = quad(
                [ndc(ow, oh, x0, y0), ndc(ow, oh, x0 + w, y0), ndc(ow, oh, x0, y0 + h), ndc(ow, oh, x0 + w, y0 + h)],
                cf.opacity,
            );
            self.queue.write_buffer(&sp.buf, 0, bytemuck::bytes_of(&s));
        }

        // Keycap overlay: bottom centre of the frame (above a bottom
        // caption), outside the camera.
        let keys = frame.keys.as_ref().filter(|k| k.opacity > 0.0 && !k.keys.is_empty());
        if let Some(ko) = keys {
            let key_h = (oh as f64 * 0.075 * ko.size).round().max(12.0) as u32;
            let id = format!("{}|{key_h}", ko.keys.join("+"));
            if self.keycaps.as_ref().map(|(k, _)| k) != Some(&id) {
                let img = keycaps::render(&ko.keys, key_h);
                let sp = self.make_sprite(&img.rgba, img.width, img.height, (img.width as f32, img.height as f32), (0.0, 0.0));
                self.keycaps = Some((id, sp));
            }
            let (_, sp) = self.keycaps.as_ref().unwrap();
            let (w, h) = (sp.size.0 as f64 * ko.scale, sp.size.1 as f64 * ko.scale);
            let bottom = if caption.is_some() { caption_top - oh as f64 * 0.015 } else { oh as f64 * 0.94 };
            let (cx, cy) = (ow as f64 * 0.5, bottom - sp.size.1 as f64 * 0.5);
            let (x0, y0) = (cx - w * 0.5, cy - h * 0.5);
            let s = quad(
                [ndc(ow, oh, x0, y0), ndc(ow, oh, x0 + w, y0), ndc(ow, oh, x0, y0 + h), ndc(ow, oh, x0 + w, y0 + h)],
                ko.opacity,
            );
            self.queue.write_buffer(&sp.buf, 0, bytemuck::bytes_of(&s));
        }

        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = begin_pass(&mut enc, target);
            if let Some(bind) = &self.scene_bind {
                pass.set_pipeline(&self.scene_pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.draw(0..3, 0..1);
                if let (Some(_), Some(sp)) = (cursor, sprite) {
                    pass.set_pipeline(&self.sprite_pipeline);
                    pass.set_bind_group(0, &sp.bind, &[]);
                    pass.draw(0..4, 0..1);
                }
                if let (Some(_), Some(bind)) = (camera, &self.camera_bind) {
                    pass.set_pipeline(&self.camera_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.draw(0..4, 0..1);
                }
                if let (Some(_), Some((_, sp))) = (caption, &self.caption) {
                    pass.set_pipeline(&self.sprite_pipeline);
                    pass.set_bind_group(0, &sp.bind, &[]);
                    pass.draw(0..4, 0..1);
                }
                if let (Some(_), Some((_, sp))) = (keys, &self.keycaps) {
                    pass.set_pipeline(&self.sprite_pipeline);
                    pass.set_bind_group(0, &sp.bind, &[]);
                    pass.draw(0..4, 0..1);
                }
            }
        }
        self.queue.submit([enc.finish()])
    }
}

fn ndc(ow: f32, oh: f32, x: f64, y: f64) -> [f32; 2] {
    [(x / ow as f64 * 2.0 - 1.0) as f32, (1.0 - y / oh as f64 * 2.0) as f32]
}

fn quad(c: [[f32; 2]; 4], opacity: f64) -> SpriteUniform {
    SpriteUniform {
        a: [c[0][0], c[0][1], c[1][0], c[1][1]],
        b: [c[2][0], c[2][1], c[3][0], c[3][1]],
        tint: [1.0, 1.0, 1.0, opacity as f32],
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

/// How many sub-samples the motion blur needs: about one per output pixel of
/// blur length at the frame's corners and centre.
fn blur_samples(layout: &Layout, frame: &FrameState, edit: &EditConfig, out_h: f64) -> u32 {
    let (a, b) = (frame.blur_from, frame.blur_to);
    if a == b {
        return 1;
    }
    let mut max_px: f64 = 0.0;
    for (ux, uy) in [(0.0, 0.0), (layout.aspect, 0.0), (0.0, 1.0), (layout.aspect, 1.0), (layout.aspect * 0.5, 0.5)] {
        let (ox, oy) = (ux - layout.aspect * 0.5, uy - 0.5);
        let dz = 1.0 / b.scale - 1.0 / a.scale;
        let dx = ox * dz * edit.zoom_blur + (b.cx - a.cx) * edit.pan_blur;
        let dy = oy * dz * edit.zoom_blur + (b.cy - a.cy) * edit.pan_blur;
        max_px = max_px.max((dx * dx + dy * dy).sqrt() * frame.camera.scale * out_h);
    }
    if max_px < 1.0 { 1 } else { (max_px.ceil() as u32).clamp(2, 48) }
}
