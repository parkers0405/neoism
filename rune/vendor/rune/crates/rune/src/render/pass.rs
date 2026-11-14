//! The frame pass a renderer records into, and how it splits at a backdrop slot: the pass
//! ends, its target is copied into a pooled texture, and a new pass re-opens with `LoadOp::Load`.
use super::*;

pub(super) struct Backdrop {
    pub(super) format: wgpu::TextureFormat,
    pub(super) texture: wgpu::Texture,
    pub(super) view: wgpu::TextureView,
}

pub(super) enum PassInner<'a, 'p> {
    Borrowed(&'a mut wgpu::RenderPass<'p>),
    Owned(wgpu::RenderPass<'p>),
}

pub(super) struct ActivePass<'a, 'p> {
    pub(super) inner: Option<PassInner<'a, 'p>>,
    pub(super) encoder: Option<(&'a mut wgpu::CommandEncoder, &'a wgpu::TextureView)>,
    /// The scissor a scope set (`None`: the whole target), restored when the pass re-opens.
    scissor: Option<[u32; 4]>,
    /// The frame pass's timestamp writes: a re-opened pass writes the end index again.
    timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>,
}

impl<'a, 'p> ActivePass<'a, 'p> {
    pub(super) fn borrowed(pass: &'a mut wgpu::RenderPass<'p>) -> Self {
        ActivePass { inner: Some(PassInner::Borrowed(pass)), encoder: None, scissor: None, timestamps: None }
    }

    pub(super) fn owned(
        pass: wgpu::RenderPass<'static>,
        encoder: &'a mut wgpu::CommandEncoder,
        view: &'a wgpu::TextureView,
    ) -> Self {
        ActivePass { inner: Some(PassInner::Owned(pass)), encoder: Some((encoder, view)), scissor: None, timestamps: None }
    }

    pub(super) fn with_timestamps(mut self, timestamps: Option<wgpu::RenderPassTimestampWrites<'a>>) -> Self {
        self.timestamps = timestamps;
        self
    }

    /// Sets the scissor to `rect` (`[x, y, width, height]`), or to the whole `size` target.
    pub(super) fn set_scissor(&mut self, rect: Option<[u32; 4]>, size: [u32; 2]) {
        self.scissor = rect;
        let [x, y, w, h] = rect.unwrap_or([0, 0, size[0], size[1]]);
        self.set_scissor_rect(x, y, w, h);
    }

    pub(super) fn scissor(&self) -> Option<[u32; 4]> { self.scissor }

    pub(super) fn split_for_backdrop<'r>(
        &mut self,
        renderer: &'r Renderer,
        at: u32,
        instance: &Instance,
        format: wgpu::TextureFormat,
        origin: [f32; 2],
        size: [u32; 2],
        in_layer: bool,
        hook: &mut EffectBackdropHook<'_>,
    ) -> Option<&'r wgpu::TextureView> {
        let bd = self.split(renderer, format, origin, size, in_layer, None, |encoder, bd| {
            let site = EffectSite {
                index: at,
                instance,
                mark: super::effect_mark(instance),
                data: instance.uv,
                scale: instance.params[3],
                format,
                origin,
                size,
                in_layer,
                srgb_blend: renderer.srgb_blend,
                backdrop: Some(&bd.view),
            };
            hook(EffectPhase::Backdrop(encoder), &site);
        })?;
        Some(&bd.view)
    }

    /// Ends the pass, copies its target into the pooled backdrop of `format` (all of it, or
    /// only `copy`, a window device-pixel rect `[x0, y0, x1, y1]`), runs `then` on the encoder,
    /// and re-opens the pass with `LoadOp::Load`. `None` when the pass has no encoder, its
    /// target lacks `COPY_SRC`, or no backdrop of `format` is pooled.
    pub(super) fn split<'r>(
        &mut self,
        renderer: &'r Renderer,
        format: wgpu::TextureFormat,
        origin: [f32; 2],
        size: [u32; 2],
        in_layer: bool,
        copy: Option<[f32; 4]>,
        then: impl FnOnce(&mut wgpu::CommandEncoder, &'r Backdrop),
    ) -> Option<&'r Backdrop> {
        let (encoder, view) = self.encoder.as_mut()?;
        if !view.texture().usage().contains(wgpu::TextureUsages::COPY_SRC) { return None; }
        let bd = renderer.backdrops.iter().find(|b| b.format == format)?;
        drop(self.inner.take());
        let (vw, vh) = (bd.texture.width(), bd.texture.height());
        if in_layer {
            let _clear = (*encoder).begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("backdrop clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &bd.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let src_tex = view.texture();
        let (src_x, src_y, dst_x, dst_y, copy_w, copy_h) = match copy {
            None => {
                let src_x = (-origin[0]).max(0.0) as u32;
                let src_y = (-origin[1]).max(0.0) as u32;
                let dst_x = origin[0].max(0.0) as u32;
                let dst_y = origin[1].max(0.0) as u32;
                let copy_w = size[0].min(src_tex.width()).saturating_sub(src_x).min(vw.saturating_sub(dst_x));
                let copy_h = size[1].min(src_tex.height()).saturating_sub(src_y).min(vh.saturating_sub(dst_y));
                (src_x, src_y, dst_x, dst_y, copy_w, copy_h)
            }
            Some(r) => {
                let span = |lo: f32, hi: f32, o: f32, src: u32, dst: u32| {
                    let lo = lo.floor().max(o.ceil()).max(0.0);
                    let hi = hi.ceil().min(o + src as f32).min(dst as f32);
                    (lo, (hi - lo).max(0.0) as u32)
                };
                let (x, w) = span(r[0], r[2], origin[0], size[0].min(src_tex.width()), vw);
                let (y, h) = span(r[1], r[3], origin[1], size[1].min(src_tex.height()), vh);
                ((x - origin[0].ceil()) as u32, (y - origin[1].ceil()) as u32, x as u32, y as u32, w, h)
            }
        };
        if copy_w > 0 && copy_h > 0 {
            (*encoder).copy_texture_to_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: src_tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: src_x, y: src_y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &bd.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: dst_x, y: dst_y, z: 0 },
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::Extent3d { width: copy_w, height: copy_h, depth_or_array_layers: 1 },
            );
        }
        then(&mut **encoder, bd);
        let mut next = (*encoder).begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(if in_layer { "isolated group" } else { "frame" }),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: *view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: self.timestamps.as_ref().filter(|t| t.end_of_pass_write_index.is_some()).map(|t| wgpu::RenderPassTimestampWrites {
                query_set: t.query_set,
                beginning_of_pass_write_index: None,
                end_of_pass_write_index: t.end_of_pass_write_index,
            }),
            occlusion_query_set: None,
            multiview_mask: None,
        }).forget_lifetime();
        next.set_viewport(0.0, 0.0, size[0] as f32, size[1] as f32, 0.0, 1.0);
        if let Some([x, y, w, h]) = self.scissor { next.set_scissor_rect(x, y, w, h); }
        self.inner = Some(PassInner::Owned(next));
        Some(bd)
    }
}

impl<'p> std::ops::Deref for ActivePass<'_, 'p> {
    type Target = wgpu::RenderPass<'p>;
    fn deref(&self) -> &Self::Target {
        match self.inner.as_ref().expect("active pass") {
            PassInner::Borrowed(p) => p,
            PassInner::Owned(p) => p,
        }
    }
}

impl<'p> std::ops::DerefMut for ActivePass<'_, 'p> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        match self.inner.as_mut().expect("active pass") {
            PassInner::Borrowed(p) => p,
            PassInner::Owned(p) => p,
        }
    }
}

impl Renderer {
    pub(super) fn ensure_backdrop(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat, (width, height): (u32, u32)) {
        let (width, height) = (width.max(1), height.max(1));
        if let Some(b) = self.backdrops.iter().find(|b| b.format == format)
            && b.texture.width() == width && b.texture.height() == height
        {
            return;
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("effect backdrop"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        if let Some(blur) = &mut self.backdrop_blur { blur.bind_backdrop(device, format, &view); }
        if let Some(b) = self.backdrops.iter_mut().find(|b| b.format == format) {
            *b = Backdrop { format, texture, view };
        } else {
            self.backdrops.push(Backdrop { format, texture, view });
        }
    }
}
