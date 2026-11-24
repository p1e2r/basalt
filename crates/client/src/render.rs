//! wgpu renderer: sky, chunk layers, entity boxes, HUD. Works with a window
//! surface or fully offscreen (for --screenshot).
use crate::assets::Assets;
use crate::mesh::{MeshData, Vertex};
use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Uniforms {
    pub vp: [[f32; 4]; 4], pub inv_vp: [[f32; 4]; 4], pub cam: [f32; 4], pub fog_color: [f32; 4], pub sky_color: [f32; 4], pub params: [f32; 4], pub screen: [f32; 4], pub sun: [f32; 4],
}

pub struct GpuMesh { pub vb: wgpu::Buffer, pub ib: wgpu::Buffer, pub count: u32 }
pub struct SectionGpu { pub layers: [Option<GpuMesh>; 3] }

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: Option<wgpu::Surface<'static>>,
    pub config: Option<wgpu::SurfaceConfiguration>,
    pub format: wgpu::TextureFormat,
    pub size: (u32, u32),
    offscreen: Option<wgpu::Texture>,
    depth: wgpu::TextureView,
    ubuf: wgpu::Buffer,
    ubind: wgpu::BindGroup,
    atlas_bind: wgpu::BindGroup,
    ui_bind: wgpu::BindGroup,
    skin_bind: wgpu::BindGroup,
    /// server list favicons: 8x4 slots of 64x64 (`set_icon`)
    icon_tex: wgpu::Texture,
    icon_bind: wgpu::BindGroup,
    p_opaque: wgpu::RenderPipeline,
    p_cutout: wgpu::RenderPipeline,
    p_trans: wgpu::RenderPipeline,
    p_crack: wgpu::RenderPipeline,
    p_sky: wgpu::RenderPipeline,
    p_ui: wgpu::RenderPipeline,
    p_blur: wgpu::RenderPipeline,
    p_pano: wgpu::RenderPipeline,
    post_layout: wgpu::BindGroupLayout,
    lin_sampler: wgpu::Sampler,
    ubuf_h: wgpu::Buffer, ubuf_v: wgpu::Buffer, ubuf_p: wgpu::Buffer,
    pano_layout: wgpu::BindGroupLayout,
    pano_bind: Option<wgpu::BindGroup>,
    post: Option<Post>,
    pub sections: HashMap<(i32, i32, i32), SectionGpu>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Tex { Ui, Blocks, Entities, Icons }

/// What is behind a menu: nothing special, the world blurred, or the title panorama (blurred).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Backdrop { None, Blur, Panorama { yaw: f32, pitch: f32 } }

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PostU { dir: [f32; 4], rot: [[f32; 4]; 4] }

/// Scene-sized targets for the backdrop blur (A: scene, B: after the horizontal pass).
struct Post { size: (u32, u32), a: wgpu::TextureView, b: wgpu::TextureView, read_a: wgpu::BindGroup, read_b: wgpu::BindGroup }

pub struct Frame<'a> {
    pub uniforms: Uniforms,
    pub cam: Vec3,
    pub vp: Mat4,
    pub render_dist: f32,
    /// world y of section index 0 (section keys are (cx, index, cz))
    pub min_y: f32,
    /// world-space extras drawn with the opaque pipeline (block atlas): outlines, fallback boxes
    pub extra: &'a MeshData,
    /// entity models drawn with the entity texture atlas (cutout)
    pub skinned: &'a MeshData,
    /// dropped items etc. in the block atlas (cutout)
    pub items3d: &'a MeshData,
    /// block-breaking cracks (block atlas, alpha blended)
    pub overlay: &'a MeshData,
    /// first-person arm (entity atlas) and held item (block atlas), drawn after the world over a
    /// cleared depth buffer so they never clip into nearby blocks
    pub hand_skin: &'a MeshData,
    pub hand_items: &'a MeshData,
    /// 2D layers drawn in order, each with its texture
    pub ui_layers: &'a [(MeshData, Tex)],
    /// the first `hud_layers` of `ui_layers` are the HUD: blurred along with the world behind menus
    pub hud_layers: usize,
    pub backdrop: Backdrop,
    /// menu background blur setting 0..10 (vanilla default 5)
    pub blur: f32,
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device.create_texture(&wgpu::TextureDescriptor { label: Some("depth"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
        dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] }).create_view(&Default::default())
}

/// The block atlas with vanilla's default 4 mipmap levels (box filtered, see assets::mip_chain).
fn upload_mipped(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
    let mips = crate::assets::mip_chain(w, h, rgba, 4);
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let t = device.create_texture(&wgpu::TextureDescriptor { label: Some("block atlas"), size, mip_level_count: 1 + mips.len() as u32, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
    let levels = std::iter::once((w, h, rgba)).chain(mips.iter().map(|(a, b, c)| (*a, *b, c.as_slice())));
    for (lvl, (lw, lh, data)) in levels.enumerate() {
        queue.write_texture(wgpu::ImageCopyTexture { texture: &t, mip_level: lvl as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, data,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: Some(lh) }, wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 });
    }
    t.create_view(&Default::default())
}

fn upload_tex(device: &wgpu::Device, queue: &wgpu::Queue, w: u32, h: u32, rgba: &[u8]) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let t = device.create_texture(&wgpu::TextureDescriptor { label: None, size, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
    queue.write_texture(wgpu::ImageCopyTexture { texture: &t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All }, rgba,
        wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) }, size);
    t.create_view(&Default::default())
}

impl Gpu {
    /// `window`: Some → render to its surface; None → offscreen target of `size`.
    pub fn new(window: Option<std::sync::Arc<winit::window::Window>>, size: (u32, u32), assets: &Assets, ui_atlas: (&[u8], u32, u32), skin: (&[u8], u32, u32)) -> Result<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
        let surface = match window { Some(w) => Some(instance.create_surface(w)?), None => None };
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface: surface.as_ref(), force_fallback_adapter: false }))
            .or_else(|| pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::LowPower, compatible_surface: surface.as_ref(), force_fallback_adapter: true })))
            .ok_or_else(|| anyhow!("no graphics adapter found"))?;
        log::info!("adapter: {:?}", adapter.get_info());
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { label: None, required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()), memory_hints: Default::default() }, None))?;
        let (format, config) = match &surface {
            Some(s) => {
                let caps = s.get_capabilities(&adapter);
                let format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
                let config = wgpu::SurfaceConfiguration { usage: wgpu::TextureUsages::RENDER_ATTACHMENT, format, width: size.0.max(1), height: size.1.max(1),
                    present_mode: wgpu::PresentMode::AutoVsync, alpha_mode: caps.alpha_modes[0], view_formats: vec![], desired_maximum_frame_latency: 2 };
                s.configure(&device, &config);
                (format, Some(config))
            }
            None => (wgpu::TextureFormat::Rgba8Unorm, None),
        };
        let offscreen = if surface.is_none() {
            Some(device.create_texture(&wgpu::TextureDescriptor { label: Some("offscreen"), size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] }))
        } else { None };
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("shader"), source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()) });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("uniforms"), size: std::mem::size_of::<Uniforms>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ulayout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }] });
        let ubind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &ulayout, entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }] });
        let tlayout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }] });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, ..Default::default() });
        let tex_bind = |view: &wgpu::TextureView| device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &tlayout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }] });
        // blocks: nearest texels, linear between mip levels (no shimmer on distant grass)
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Nearest, min_filter: wgpu::FilterMode::Nearest, mipmap_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let atlas_view = upload_mipped(&device, &queue, assets.atlas.w, assets.atlas.h, &assets.atlas.rgba);
        let atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &tlayout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&atlas_view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&mip_sampler) }] });
        let ui_bind = tex_bind(&upload_tex(&device, &queue, ui_atlas.1, ui_atlas.2, ui_atlas.0));
        let skin_bind = tex_bind(&upload_tex(&device, &queue, skin.1, skin.2, skin.0));
        let icon_tex = device.create_texture(&wgpu::TextureDescriptor { label: Some("icons"), size: wgpu::Extent3d { width: 512, height: 256, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        let icon_bind = tex_bind(&icon_tex.create_view(&Default::default()));
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&ulayout, &tlayout], push_constant_ranges: &[] });
        let vattrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Unorm8x4, 3 => Unorm8x4];
        let vlayout = wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &vattrs };
        let mk = |vs: &str, fs: &str, blend: Option<wgpu::BlendState>, depth_write: bool, depth_test: bool, cull: bool, buffers: &[wgpu::VertexBufferLayout]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some(fs), layout: Some(&layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: vs, buffers, compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &shader, entry_point: fs, targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                primitive: wgpu::PrimitiveState { cull_mode: if cull { Some(wgpu::Face::Back) } else { None }, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: depth_write,
                    depth_compare: if depth_test { wgpu::CompareFunction::LessEqual } else { wgpu::CompareFunction::Always }, stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(), multiview: None, cache: None })
        };
        let p_opaque = mk("vs_chunk", "fs_opaque", None, true, true, true, &[vlayout.clone()]);
        let p_cutout = mk("vs_chunk", "fs_cutout", None, true, true, false, &[vlayout.clone()]);
        let p_trans = mk("vs_chunk", "fs_trans", Some(wgpu::BlendState::ALPHA_BLENDING), false, true, false, &[vlayout.clone()]);
        // block-breaking cracks: result = 2 · src · dst (darkens and lightens like the vanilla overlay)
        let mul = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::Dst, dst_factor: wgpu::BlendFactor::Src, operation: wgpu::BlendOperation::Add };
        let p_crack = mk("vs_chunk", "fs_crack", Some(wgpu::BlendState { color: mul, alpha: wgpu::BlendComponent::OVER }), false, true, false, &[vlayout.clone()]);
        let p_sky = mk("vs_sky", "fs_sky", None, false, false, false, &[]);
        let p_ui = mk("vs_ui", "fs_ui", Some(wgpu::BlendState::ALPHA_BLENDING), false, false, false, &[vlayout.clone()]);
        let depth = make_depth(&device, size.0.max(1), size.1.max(1));
        // menu backdrops (post.wgsl / pano.wgsl): fullscreen passes without depth
        let post_entry = |dim: wgpu::TextureViewDimension| [
            wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: dim, multisampled: false }, count: None },
            wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None }];
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("post"), entries: &post_entry(wgpu::TextureViewDimension::D2) });
        let pano_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pano"), entries: &post_entry(wgpu::TextureViewDimension::D2Array) });
        let full = |src: &str, fs: &str, bl: &wgpu::BindGroupLayout| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(fs), source: wgpu::ShaderSource::Wgsl(src.into()) });
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[bl], push_constant_ranges: &[] });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor { label: Some(fs), layout: Some(&pl),
                vertex: wgpu::VertexState { module: &module, entry_point: "vs_full", buffers: &[], compilation_options: Default::default() },
                fragment: Some(wgpu::FragmentState { module: &module, entry_point: fs, targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
                primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None, cache: None })
        };
        let p_blur = full(include_str!("post.wgsl"), "fs_blur", &post_layout);
        let p_pano = full(include_str!("pano.wgsl"), "fs_pano", &pano_layout);
        let lin_sampler = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        let mkbuf = || device.create_buffer(&wgpu::BufferDescriptor { label: Some("post"), size: std::mem::size_of::<PostU>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let (ubuf_h, ubuf_v, ubuf_p) = (mkbuf(), mkbuf(), mkbuf());
        Ok(Gpu { device, queue, surface, config, format, size, offscreen, depth, ubuf, ubind, atlas_bind, ui_bind, skin_bind, icon_tex, icon_bind, p_opaque, p_cutout, p_trans, p_crack, p_sky, p_ui,
            p_blur, p_pano, post_layout, lin_sampler, ubuf_h, ubuf_v, ubuf_p, pano_layout, pano_bind: None, post: None, sections: HashMap::new() })
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 { return; }
        self.size = (w, h);
        if let (Some(s), Some(c)) = (&self.surface, &mut self.config) { c.width = w; c.height = h; s.configure(&self.device, c); }
        self.depth = make_depth(&self.device, w, h);
    }

    /// Six equally sized RGBA faces (front, right, back, left, up, down) for the title backdrop.
    pub fn set_panorama(&mut self, faces: &[(u32, u32, &[u8])]) {
        let Some(&(w, h, _)) = faces.first() else { return };
        if faces.len() != 6 || faces.iter().any(|f| f.0 != w || f.1 != h) { return; }
        let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 6 };
        let t = self.device.create_texture(&wgpu::TextureDescriptor { label: Some("panorama"), size, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST, view_formats: &[] });
        for (i, f) in faces.iter().enumerate() {
            self.queue.write_texture(wgpu::ImageCopyTexture { texture: &t, mip_level: 0, origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 }, aspect: wgpu::TextureAspect::All }, f.2,
                wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w), rows_per_image: Some(h) }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
        }
        let view = t.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D2Array), ..Default::default() });
        self.pano_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.pano_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: self.ubuf_p.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&view) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.lin_sampler) }] }));
    }

    /// Put a 64x64 RGBA image into icon slot `i` (0..32); UV rect is `icon_uv(i)`.
    pub fn set_icon(&mut self, i: usize, rgba: &[u8]) {
        if i >= 32 || rgba.len() != 64 * 64 * 4 { return; }
        self.queue.write_texture(wgpu::ImageCopyTexture { texture: &self.icon_tex, mip_level: 0, origin: wgpu::Origin3d { x: (i % 8) as u32 * 64, y: (i / 8) as u32 * 64, z: 0 }, aspect: wgpu::TextureAspect::All }, rgba,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(256), rows_per_image: Some(64) }, wgpu::Extent3d { width: 64, height: 64, depth_or_array_layers: 1 });
    }

    fn ensure_post(&mut self) {
        if self.post.as_ref().map(|p| p.size == self.size).unwrap_or(false) { return; }
        let (w, h) = (self.size.0.max(1), self.size.1.max(1));
        let mk = || self.device.create_texture(&wgpu::TextureDescriptor { label: Some("post"), size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
            dimension: wgpu::TextureDimension::D2, format: self.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[] }).create_view(&Default::default());
        let (a, b) = (mk(), mk());
        let bind = |buf: &wgpu::Buffer, v: &wgpu::TextureView| self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.post_layout, entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(v) },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.lin_sampler) }] });
        let (read_a, read_b) = (bind(&self.ubuf_h, &a), bind(&self.ubuf_v, &b));
        self.post = Some(Post { size: self.size, a, b, read_a, read_b });
    }

    fn upload(&self, m: &MeshData) -> Option<GpuMesh> {
        if m.idx.is_empty() { return None; }
        Some(GpuMesh {
            vb: self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&m.verts), usage: wgpu::BufferUsages::VERTEX }),
            ib: self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&m.idx), usage: wgpu::BufferUsages::INDEX }),
            count: m.idx.len() as u32,
        })
    }

    pub fn set_section(&mut self, key: (i32, i32, i32), mesh: &crate::mesh::SectionMesh) {
        let s = SectionGpu { layers: [self.upload(&mesh.layers[0]), self.upload(&mesh.layers[1]), self.upload(&mesh.layers[2])] };
        if s.layers.iter().all(|l| l.is_none()) { self.sections.remove(&key); } else { self.sections.insert(key, s); }
    }

    fn draw(&self, view: &wgpu::TextureView, f: &Frame) {
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&f.uniforms));
        // visible sections: within distance and in front-ish of the camera frustum
        let planes = frustum_planes(&f.vp);
        let cull = std::env::var("BASALT_NOCULL").is_err();
        let mut vis: Vec<(f32, &SectionGpu)> = Vec::new();
        for (k, s) in &self.sections {
            let c = Vec3::new(k.0 as f32 * 16.0 + 8.0, f.min_y + k.1 as f32 * 16.0 + 8.0, k.2 as f32 * 16.0 + 8.0) - f.cam;
            let hd = (c.x * c.x + c.z * c.z).sqrt();
            if hd > f.render_dist + 24.0 { continue; }
            if cull && planes.iter().any(|p| p.truncate().dot(c) + p.w < -14.0) { continue; }
            vis.push((c.length_squared(), s));
        }
        vis.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        let extra = self.upload(f.extra); let skinned = self.upload(f.skinned); let items3d = self.upload(f.items3d); let overlay = self.upload(f.overlay);
        let (hand_skin, hand_items) = (self.upload(f.hand_skin), self.upload(f.hand_items));
        let layers: Vec<(Option<GpuMesh>, Tex)> = f.ui_layers.iter().map(|(m, t)| (self.upload(m), *t)).collect();
        let mut enc = self.device.create_command_encoder(&Default::default());
        let post = if f.backdrop != Backdrop::None { self.post.as_ref() } else { None };
        let scene: &wgpu::TextureView = post.map(|p| &p.a).unwrap_or(view);
        let pano = matches!(f.backdrop, Backdrop::Panorama { .. }) && post.is_some();
        if pano {
            if let Backdrop::Panorama { yaw, pitch } = f.backdrop {
                let rot = Mat4::from_rotation_y(-yaw.to_radians()) * Mat4::from_rotation_x(-pitch.to_radians());
                let (w, h) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
                self.queue.write_buffer(&self.ubuf_p, 0, bytemuck::bytes_of(&PostU { dir: [(85f32.to_radians() / 2.0).tan(), w / h, 0.0, 0.0], rot: rot.to_cols_array_2d() }));
            }
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: scene, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.05, b: 0.05, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None });
            if let Some(b) = &self.pano_bind { rp.set_pipeline(&self.p_pano); rp.set_bind_group(0, b, &[]); rp.draw(0..3, 0..1); }
        }
        if !pano {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: scene, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None });
            rp.set_bind_group(0, &self.ubind, &[]);
            rp.set_bind_group(1, &self.atlas_bind, &[]);
            rp.set_pipeline(&self.p_sky); rp.draw(0..3, 0..1);
            let draw_mesh = |rp: &mut wgpu::RenderPass, m: &GpuMesh| { rp.set_vertex_buffer(0, m.vb.slice(..)); rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32); rp.draw_indexed(0..m.count, 0, 0..1); };
            rp.set_pipeline(&self.p_opaque);
            for (_, s) in &vis { if let Some(m) = &s.layers[0] { draw_mesh(&mut rp, m); } }
            if let Some(m) = &extra { draw_mesh(&mut rp, m); }
            if let Some(m) = &skinned { rp.set_pipeline(&self.p_cutout); rp.set_bind_group(1, &self.skin_bind, &[]); draw_mesh(&mut rp, m); rp.set_bind_group(1, &self.atlas_bind, &[]); }
            rp.set_pipeline(&self.p_cutout);
            for (_, s) in &vis { if let Some(m) = &s.layers[1] { draw_mesh(&mut rp, m); } }
            if let Some(m) = &items3d { draw_mesh(&mut rp, m); }
            if let Some(m) = &overlay { rp.set_pipeline(&self.p_crack); draw_mesh(&mut rp, m); }
            rp.set_pipeline(&self.p_trans);
            for (_, s) in vis.iter().rev() { if let Some(m) = &s.layers[2] { draw_mesh(&mut rp, m); } }
        }
        if !pano {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: scene, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None });
            rp.set_bind_group(0, &self.ubind, &[]);
            let draw_mesh = |rp: &mut wgpu::RenderPass, m: &GpuMesh| { rp.set_vertex_buffer(0, m.vb.slice(..)); rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32); rp.draw_indexed(0..m.count, 0, 0..1); };
            rp.set_pipeline(&self.p_cutout);
            if let Some(m) = &hand_skin { rp.set_bind_group(1, &self.skin_bind, &[]); draw_mesh(&mut rp, m); }
            if let Some(m) = &hand_items { rp.set_bind_group(1, &self.atlas_bind, &[]); draw_mesh(&mut rp, m); }
        }
        // the HUD belongs to the scene that gets blurred
        let first_ui = if post.is_some() && !pano {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: scene, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None });
            rp.set_bind_group(0, &self.ubind, &[]);
            rp.set_pipeline(&self.p_ui);
            for (m, t) in layers.iter().take(f.hud_layers) {
                let Some(m) = m else { continue };
                rp.set_bind_group(1, match t { Tex::Ui => &self.ui_bind, Tex::Blocks => &self.atlas_bind, Tex::Entities => &self.skin_bind, Tex::Icons => &self.icon_bind }, &[]);
                rp.set_vertex_buffer(0, m.vb.slice(..)); rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32); rp.draw_indexed(0..m.count, 0, 0..1);
            }
            f.hud_layers
        } else { 0 };
        // backdrop: blur the scene (A) horizontally into B, then vertically into the output
        if let Some(p) = post {
            let (w, h) = (self.size.0.max(1) as f32, self.size.1.max(1) as f32);
            let sigma = h / 200.0 * f.blur / 5.0;
            self.queue.write_buffer(&self.ubuf_h, 0, bytemuck::bytes_of(&PostU { dir: [1.0 / w, 0.0, sigma, 1.0], rot: Mat4::IDENTITY.to_cols_array_2d() }));
            self.queue.write_buffer(&self.ubuf_v, 0, bytemuck::bytes_of(&PostU { dir: [0.0, 1.0 / h, sigma, 1.0], rot: Mat4::IDENTITY.to_cols_array_2d() }));
            for (src, dst) in [(&p.read_a, &p.b), (&p.read_b, view)] {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: dst, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                    depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None });
                rp.set_pipeline(&self.p_blur); rp.set_bind_group(0, src, &[]); rp.draw(0..3, 0..1);
            }
        }
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                timestamp_writes: None, occlusion_query_set: None });
            let draw_mesh = |rp: &mut wgpu::RenderPass, m: &GpuMesh| { rp.set_vertex_buffer(0, m.vb.slice(..)); rp.set_index_buffer(m.ib.slice(..), wgpu::IndexFormat::Uint32); rp.draw_indexed(0..m.count, 0, 0..1); };
            rp.set_bind_group(0, &self.ubind, &[]);
            rp.set_pipeline(&self.p_ui);
            for (m, t) in layers.iter().skip(first_ui) {
                let Some(m) = m else { continue };
                rp.set_bind_group(1, match t { Tex::Ui => &self.ui_bind, Tex::Blocks => &self.atlas_bind, Tex::Entities => &self.skin_bind, Tex::Icons => &self.icon_bind }, &[]);
                draw_mesh(&mut rp, m);
            }
        }
        self.queue.submit([enc.finish()]);
    }

    /// Render to the window surface.
    pub fn render(&mut self, f: &Frame) -> Result<()> {
        if f.backdrop != Backdrop::None { self.ensure_post(); }
        let Some(surface) = &self.surface else { return Ok(()) };
        let tex = match surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => { if let Some(c) = &self.config { surface.configure(&self.device, c); } return Ok(()); }
        };
        let view = tex.texture.create_view(&Default::default());
        self.draw(&view, f);
        tex.present();
        Ok(())
    }

    /// Render to the offscreen target and wait for the GPU (benchmarks).
    pub fn render_offscreen(&mut self, f: &Frame) {
        if f.backdrop != Backdrop::None { self.ensure_post(); }
        if let Some(t) = &self.offscreen { let view = t.create_view(&Default::default()); self.draw(&view, f); self.device.poll(wgpu::Maintain::Wait); }
    }
    /// Bytes of vertex + index buffers held for chunk sections.
    pub fn section_bytes(&self) -> u64 {
        self.sections.values().flat_map(|s| s.layers.iter().flatten()).map(|m| m.vb.size() + m.ib.size()).sum()
    }

    /// Render offscreen and write a PNG.
    pub fn screenshot(&mut self, f: &Frame, path: &str) -> Result<()> { let rgb = self.capture(f)?; save_png(path, self.size.0, self.size.1, &rgb) }

    /// Render offscreen and read back the frame as tightly packed RGB.
    pub fn capture(&mut self, f: &Frame) -> Result<Vec<u8>> {
        if f.backdrop != Backdrop::None { self.ensure_post(); }
        let (w, h) = self.size;
        let owned;
        let tex = match &self.offscreen { Some(t) => t, None => {
            owned = self.device.create_texture(&wgpu::TextureDescriptor { label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 }, mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2, format: self.format, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
            &owned } };
        let view = tex.create_view(&Default::default());
        self.draw(&view, f);
        let bpr = (w * 4).div_ceil(256) * 256;
        let buf = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (bpr * h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::ImageCopyBuffer { buffer: &buf, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h) } }, wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 });
        self.queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        self.device.poll(wgpu::Maintain::Wait);
        let data = slice.get_mapped_range();
        let bgra = matches!(self.format, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb);
        let mut out = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h { for x in 0..w { let i = (y * bpr + x * 4) as usize; if bgra { out.extend_from_slice(&[data[i + 2], data[i + 1], data[i]]); } else { out.extend_from_slice(&data[i..i + 3]); } } }
        drop(data); buf.unmap();
        Ok(out)
    }
}

pub fn save_png(path: &str, w: u32, h: u32, rgb: &[u8]) -> Result<()> {
    let mut e = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(path)?), w, h);
    e.set_color(png::ColorType::Rgb); e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(rgb)?;
    Ok(())
}

fn frustum_planes(vp: &Mat4) -> [glam::Vec4; 5] {
    let r = [vp.row(0), vp.row(1), vp.row(2), vp.row(3)];
    let n = |p: glam::Vec4| p / p.truncate().length();
    [n(r[3] + r[0]), n(r[3] - r[0]), n(r[3] + r[1]), n(r[3] - r[1]), n(r[3] - r[2])]
}

/// UV rect of icon slot `i` in the icon texture (see `Gpu::set_icon`).
pub fn icon_uv(i: usize) -> [f32; 4] { let (x, y) = ((i % 8) as f32 * 64.0, (i / 8) as f32 * 64.0); [x / 512.0, y / 256.0, (x + 64.0) / 512.0, (y + 64.0) / 256.0] }
