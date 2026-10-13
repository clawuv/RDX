//! egui/wgpu YUV 视频渲染（ADR-0001 / P1 关键技术点）。
//!
//! 解码输出保持 I420 三平面，作为三张 R8Unorm 纹理直上 GPU，
//! YUV→RGB（BT.601 limited range）在片元着色器完成，CPU 侧零颜色转换。

use std::sync::{Arc, Mutex};

use egui_wgpu::CallbackTrait;
use rd_codec::DecodedFrame;
use wgpu::{
    BindGroup, BindGroupLayout, Device, Queue, RenderPass, RenderPipeline, Sampler, Texture,
    TextureFormat, TextureUsages,
};

const YUV_SHADER: &str = r#"
@group(0) @binding(0) var tex_y: texture_2d<f32>;
@group(0) @binding(1) var tex_u: texture_2d<f32>;
@group(0) @binding(2) var tex_v: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) vi: u32) -> VsOut {
    // 覆盖整个视口的大三角形，无需顶点缓冲。
    var pos = array<vec2<f32>, 3>(
        vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0)
    );
    // NDC y 向上；图像行序自上而下，底部顶点取 uv.y = 1。
    var uv = array<vec2<f32>, 3>(
        vec2(0.0, 1.0), vec2(2.0, 1.0), vec2(0.0, -1.0)
    );
    var out: VsOut;
    out.pos = vec4(pos[vi], 0.0, 1.0);
    out.uv = uv[vi];
    return out;
}

@fragment
fn fs(in: VsOut) -> @location(0) vec4<f32> {
    let y = textureSample(tex_y, samp, in.uv).r;
    let u = textureSample(tex_u, samp, in.uv).r;
    let v = textureSample(tex_v, samp, in.uv).r;
    // BT.601 limited range → 线性 RGB。
    let yy = (y - 16.0 / 255.0) * 255.0 / 219.0;
    let cb = (u - 128.0 / 255.0) * 255.0 / 224.0;
    let cr = (v - 128.0 / 255.0) * 255.0 / 224.0;
    let r = yy + 1.402 * cr;
    let g = yy - 0.344136 * cb - 0.714136 * cr;
    let b = yy + 1.772 * cb;
    return vec4(clamp(vec3(r, g, b), vec3(0.0), vec3(1.0)), 1.0);
}
"#;

struct PlanarTextures {
    width: u32,
    height: u32,
    y: Texture,
    u: Texture,
    v: Texture,
    bind_group: BindGroup,
}

/// 共享渲染资源：App 侧负责纹理上传，paint 回调侧负责绘制。
/// eframe 在同一线程驱动 UI 与渲染，锁只短暂持有且从不跨 await。
pub struct VideoShared {
    inner: Mutex<VideoInner>,
}

struct VideoInner {
    pipeline: RenderPipeline,
    layout: BindGroupLayout,
    sampler: Sampler,
    device: Device,
    queue: Queue,
    textures: Option<PlanarTextures>,
}

impl VideoShared {
    pub fn new(render_state: &egui_wgpu::RenderState) -> Arc<Self> {
        let device = render_state.device.clone();
        let queue = render_state.queue.clone();
        let target_format = render_state.target_format;

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rdx-yuv"),
            source: wgpu::ShaderSource::Wgsl(YUV_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("rdx-yuv-bgl"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                texture_entry(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("rdx-yuv-pl"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rdx-yuv-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                buffers: &[],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("rdx-yuv-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Arc::new(Self {
            inner: Mutex::new(VideoInner {
                pipeline,
                layout,
                sampler,
                device,
                queue,
                textures: None,
            }),
        })
    }

    /// 上传一帧 I420；分辨率变化时重建纹理。
    pub fn upload(&self, frame: &DecodedFrame) {
        let mut inner = self.inner.lock().expect("video state lock");
        let need_rebuild = inner
            .textures
            .as_ref()
            .is_none_or(|t| t.width != frame.width || t.height != frame.height);
        if need_rebuild {
            inner.textures = None;
            let y = create_plane(&inner.device, frame.width, frame.height, "y");
            let u = create_plane(&inner.device, frame.width / 2, frame.height / 2, "u");
            let v = create_plane(&inner.device, frame.width / 2, frame.height / 2, "v");
            let y_view = y.create_view(&wgpu::TextureViewDescriptor::default());
            let u_view = u.create_view(&wgpu::TextureViewDescriptor::default());
            let v_view = v.create_view(&wgpu::TextureViewDescriptor::default());
            let bind_group = inner.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("rdx-yuv-bg"),
                layout: &inner.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&y_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&u_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&v_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::Sampler(&inner.sampler),
                    },
                ],
            });
            inner.textures = Some(PlanarTextures {
                width: frame.width,
                height: frame.height,
                y,
                u,
                v,
                bind_group,
            });
        }

        let (w, h) = (frame.width as usize, frame.height as usize);
        let y_len = w * h;
        let uv_len = (w / 2) * (h / 2);
        let y_rows = &frame.data[..y_len];
        let u_rows = &frame.data[y_len..y_len + uv_len];
        let v_rows = &frame.data[y_len + uv_len..y_len + 2 * uv_len];
        let queue = inner.queue.clone();
        let (y, u, v) = {
            let textures = inner.textures.as_ref().expect("textures built above");
            (textures.y.clone(), textures.u.clone(), textures.v.clone())
        };
        drop(inner); // 上传本身不需要持锁。
        write_plane(&queue, &y, y_rows, w as u32);
        write_plane(&queue, &u, u_rows, (w / 2) as u32);
        write_plane(&queue, &v, v_rows, (w / 2) as u32);
    }

    pub fn is_ready(&self) -> bool {
        self.inner
            .lock()
            .expect("video state lock")
            .textures
            .is_some()
    }

    pub fn dimensions(&self) -> Option<(u32, u32)> {
        self.inner
            .lock()
            .expect("video state lock")
            .textures
            .as_ref()
            .map(|t| (t.width, t.height))
    }

    fn paint(&self, info: &egui::PaintCallbackInfo, render_pass: &mut RenderPass<'static>) {
        let inner = self.inner.lock().expect("video state lock");
        let Some(textures) = inner.textures.as_ref() else {
            return;
        };
        let viewport = info.viewport_in_pixels();
        render_pass.set_pipeline(&inner.pipeline);
        render_pass.set_bind_group(0, &textures.bind_group, &[]);
        render_pass.set_scissor_rect(
            viewport.left_px.max(0) as u32,
            viewport.top_px.max(0) as u32,
            viewport.width_px.max(1) as u32,
            viewport.height_px.max(1) as u32,
        );
        render_pass.draw(0..3, 0..1);
    }
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

fn create_plane(device: &Device, width: u32, height: u32, name: &str) -> Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(name),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TextureFormat::R8Unorm,
        usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn write_plane(queue: &Queue, texture: &Texture, rows: &[u8], bytes_per_row: u32) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        rows,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(bytes_per_row),
            rows_per_image: None,
        },
        wgpu::Extent3d {
            width: bytes_per_row,
            height: (rows.len() as u32) / bytes_per_row,
            depth_or_array_layers: 1,
        },
    );
}

/// 每帧插入 egui 绘制列表的回调；资源从 RenderState 的 callback_resources 取。
pub struct YuvPaintCallback {
    pub shared: Arc<VideoShared>,
}

impl CallbackTrait for YuvPaintCallback {
    fn paint(
        &self,
        info: egui::PaintCallbackInfo,
        render_pass: &mut RenderPass<'static>,
        _callback_resources: &egui_wgpu::CallbackResources,
    ) {
        self.shared.paint(&info, render_pass);
    }
}
