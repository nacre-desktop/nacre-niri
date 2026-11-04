use std::any::Any;
use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use niri_config::utils::MergeWith as _;
use niri_config::{Color, Config, CornerRadius, GradientInterpolation, LayerRule};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::desktop::{LayerSurface, PopupKind, PopupManager, WindowSurfaceType};
use smithay::utils::user_data::UserDataMap;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use smithay::wayland::compositor::{remove_pre_commit_hook, with_states, HookId};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, Layer};

use super::motion::{LayerMotion, MotionCurve, MotionPose, OpacityMode};
use super::reveal::{Rect as RevealRect, RevealDescriptor};
use super::ResolvedLayerRules;
use crate::animation::Clock;
use crate::layout::shadow::Shadow;
use crate::niri_render_elements;
use crate::protocols::layer_motion::{LayerMotionRuntime, MotionPlane};
use crate::render_helpers::background_effect::BackgroundEffectElement;
use crate::render_helpers::border::BorderRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::surface::push_elements_from_surface_tree;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{background_effect, RenderCtx};
use crate::utils::{baba_is_float_offset, round_logical_in_physical};

#[derive(Debug)]
pub struct MappedLayer {
    /// The surface itself.
    surface: LayerSurface,

    /// Pre-commit hook that we have on all mapped layer surfaces.
    pre_commit_hook: HookId,

    /// Up-to-date rules.
    rules: ResolvedLayerRules,

    /// Whether to recompute layer rules on the next commit.
    ///
    /// Set in the pre-commit hook when the layer changes; consumed in the commit handler.
    recompute_rules_on_commit: bool,

    /// Buffer to draw instead of the surface when it should be blocked out.
    block_out_buffer: SolidColorBuffer,

    /// The shadow around the surface.
    shadow: Shadow,

    /// The blur config, passed for background effect rendering.
    blur_config: niri_config::Blur,

    /// The view size for the layer surface's output.
    view_size: Size<f64, Logical>,

    /// Scale of the output the layer surface is on (and rounds its sizes to).
    scale: f64,

    /// Clock for driving animations.
    clock: Clock,

    /// Optional protocol-driven atlas animation shared with its controller.
    motion: Option<Arc<Mutex<LayerMotionRuntime>>>,
    promotion_motion: Option<LayerMotion>,
    reveal_texture_cache: RefCell<Vec<RevealTextureCache>>,
    reveal_edge_cache: RefCell<Option<RevealEdgeTextureCache>>,
    reveal_edge_rejected: RefCell<Option<RevealEdgeCacheKey>>,
}

#[derive(Clone, Debug, PartialEq)]
struct RevealTextureCacheKey {
    descriptor: RevealDescriptor,
    progress: u64,
    scale: u32,
    surface_width: u32,
    surface_height: u32,
    location_x: u64,
    location_y: u64,
}

struct RevealTextureCache {
    key: RevealTextureCacheKey,
    texture: Box<dyn Any>,
    shadow: Option<ShadowRenderElement>,
    crop: RevealRect,
}

#[derive(Clone, Debug, PartialEq)]
struct RevealEdgeCacheKey {
    descriptor: RevealDescriptor,
    scale: u32,
    surface_width: u32,
    surface_height: u32,
    location_x: u64,
    location_y: u64,
}

struct RevealEdgeTextureCache {
    key: RevealEdgeCacheKey,
    open: Box<dyn Any>,
    band: Box<dyn Any>,
    crop: RevealRect,
    vertical: bool,
    far: bool,
    origin: f32,
    lead: f32,
    first: i64,
    band_extent: u32,
    total_extent: u32,
}

impl std::fmt::Debug for RevealEdgeTextureCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevealEdgeTextureCache")
            .field("crop", &self.crop)
            .field("vertical", &self.vertical)
            .field("far", &self.far)
            .field("band_extent", &self.band_extent)
            .field("total_extent", &self.total_extent)
            .finish_non_exhaustive()
    }
}

// This edge-template bake follows Bead's MIT-licensed tree.rs edge_items/build_edge algorithm
// (commit 0eb9a9a8afdbd8e8a2f6770e1a3b05818af198b0; Copyright (c) 2026 siverteh).
const REVEAL_EDGE_INSIDE: f32 = 44.;
const REVEAL_EDGE_OUTSIDE: f32 = 8.;
const REVEAL_EDGE_MIN_REVEALED: f32 = REVEAL_EDGE_INSIDE + 40.;
const REVEAL_EDGE_CACHE_LIMIT: usize = 64 * 1024 * 1024;

struct RevealEdgeSpec {
    vertical: bool,
    far: bool,
    origin: f32,
    length: f32,
    full_lead: f32,
    full_body: RevealRect,
    closed_body: RevealRect,
}

struct RevealEdgePixels {
    open: tiny_skia::Pixmap,
    band: tiny_skia::Pixmap,
    crop: RevealRect,
    vertical: bool,
    far: bool,
    origin: f32,
    lead: f32,
    first: i64,
    band_extent: u32,
    total_extent: u32,
}

impl std::fmt::Debug for RevealTextureCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RevealTextureCache")
            .field("key", &self.key)
            .field("shadow", &self.shadow.is_some())
            .field("crop", &self.crop)
            .finish_non_exhaustive()
    }
}

niri_render_elements! {
    LayerSurfaceRenderElement<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        MotionPlane = MotionPlaneRenderElement<R>,
        Border = BorderRenderElement,
        Reveal = TextureRenderElement<R::NiriTextureId>,
        RevealClip = CropRenderElement<TextureRenderElement<R::NiriTextureId>>,
        SolidColor = SolidColorRenderElement,
        Shadow = ShadowRenderElement,
        BackgroundEffect = BackgroundEffectElement,
    }
}

type MotionPlaneInner<R> = CropRenderElement<
    RelocateRenderElement<CropRenderElement<RescaleRenderElement<WaylandSurfaceRenderElement<R>>>>,
>;

#[derive(Debug)]
pub struct MotionPlaneRenderElement<R: NiriRenderer> {
    inner: MotionPlaneInner<R>,
    id: Id,
    damage: crate::render_helpers::damage::ExtraDamage,
}

type LayerMotionSnapshot = (Vec<MotionPlane>, Vec<(u32, MotionPose)>);

fn to_physical_cover(
    rect: Rectangle<f64, Logical>,
    scale: Scale<f64>,
) -> Rectangle<i32, smithay::utils::Physical> {
    let left = (rect.loc.x * scale.x).floor() as i32;
    let top = (rect.loc.y * scale.y).floor() as i32;
    let right = ((rect.loc.x + rect.size.w) * scale.x).ceil() as i32;
    let bottom = ((rect.loc.y + rect.size.h) * scale.y).ceil() as i32;
    Rectangle::new(
        Point::from((left, top)),
        Size::from((right.saturating_sub(left), bottom.saturating_sub(top))),
    )
}

fn reveal_edge_spec(reveal: &RevealDescriptor) -> Option<RevealEdgeSpec> {
    if !reveal.visible
        || reveal.corner_mode != super::reveal::CornerMode::QuadraticCut
        || reveal.radii_from != reveal.radii_to
        || reveal.shadow.is_some()
    {
        return None;
    }
    let from = reveal.animated_from;
    let to = reveal.animated_to;
    if from.h >= 0. && from.h < to.h && from.w > 0. && to.h > 0. && from.x == to.x && from.w == to.w
    {
        let far = if from.y == to.y {
            true
        } else if from.y + from.h == to.y + to.h {
            false
        } else {
            return None;
        };
        return Some(RevealEdgeSpec {
            vertical: true,
            far,
            origin: reveal.crop.y,
            length: to.h,
            full_lead: if far { to.y + to.h } else { to.y },
            full_body: to,
            closed_body: if far {
                RevealRect::new(to.x, to.y, to.w, 0.)
            } else {
                RevealRect::new(to.x, to.y + to.h, to.w, 0.)
            },
        });
    }
    if from.w >= 0. && from.w < to.w && from.h > 0. && to.w > 0. && from.y == to.y && from.h == to.h
    {
        let far = if from.x == to.x {
            true
        } else if from.x + from.w == to.x + to.w {
            false
        } else {
            return None;
        };
        return Some(RevealEdgeSpec {
            vertical: false,
            far,
            origin: reveal.crop.x,
            length: to.w,
            full_lead: if far { to.x + to.w } else { to.x },
            full_body: to,
            closed_body: if far {
                RevealRect::new(to.x, to.y, 0., to.h)
            } else {
                RevealRect::new(to.x + to.w, to.y, 0., to.h)
            },
        });
    }
    None
}

fn reveal_edge_pixel(pixmap: &tiny_skia::Pixmap, x: u32, y: u32) -> [u8; 4] {
    let offset = ((y * pixmap.width() + x) * 4) as usize;
    pixmap.data()[offset..offset + 4].try_into().unwrap()
}

fn reveal_edge_template_matches(
    band: &tiny_skia::Pixmap,
    open: &tiny_skia::Pixmap,
    first: i64,
    last: i64,
    shift: i64,
    vertical: bool,
    far: bool,
) -> bool {
    let band_extent = if vertical {
        band.height()
    } else {
        band.width()
    } as i64;
    let total_extent = if vertical {
        open.height()
    } else {
        open.width()
    } as i64;
    if band_extent < 4
        || first + shift < 1
        || shift + last >= total_extent
        || (vertical && band.width() != open.width())
        || (!vertical && band.height() != open.height())
    {
        return false;
    }
    let close = |a: [u8; 4], b: [u8; 4], tolerance: i32| {
        a.iter()
            .zip(b)
            .all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() <= tolerance)
    };
    let (inner, next) = if far {
        (0, 1)
    } else {
        (band_extent - 1, band_extent - 2)
    };
    let uniform = if vertical {
        (0..band.width()).all(|x| {
            close(
                reveal_edge_pixel(band, x, inner as u32),
                reveal_edge_pixel(band, x, next as u32),
                2,
            )
        })
    } else {
        (0..band.height()).all(|y| {
            close(
                reveal_edge_pixel(band, inner as u32, y),
                reveal_edge_pixel(band, next as u32, y),
                2,
            )
        })
    };
    if !uniform {
        return false;
    }
    let within = |template: [u8; 4], around: [[u8; 4]; 3]| {
        (0..4).all(|channel| {
            let low = around
                .iter()
                .map(|pixel| i32::from(pixel[channel]))
                .min()
                .unwrap_or(0)
                - 28;
            let high = around
                .iter()
                .map(|pixel| i32::from(pixel[channel]))
                .max()
                .unwrap_or(0)
                + 28;
            (low..=high).contains(&i32::from(template[channel]))
        })
    };
    if vertical {
        (0..band.height()).all(|y| {
            (0..band.width()).step_by(3).all(|x| {
                let row = (i64::from(y) + first + shift) as u32;
                within(
                    reveal_edge_pixel(band, x, y),
                    [
                        reveal_edge_pixel(open, x, row - 1),
                        reveal_edge_pixel(open, x, row),
                        reveal_edge_pixel(open, x, row + 1),
                    ],
                )
            })
        })
    } else {
        (0..band.width()).all(|x| {
            (0..band.height()).step_by(3).all(|y| {
                let column = (i64::from(x) + first + shift) as u32;
                within(
                    reveal_edge_pixel(band, x, y),
                    [
                        reveal_edge_pixel(open, column - 1, y),
                        reveal_edge_pixel(open, column, y),
                        reveal_edge_pixel(open, column + 1, y),
                    ],
                )
            })
        })
    }
}

fn build_reveal_edge_pixels(
    reveal: &RevealDescriptor,
    spec: &RevealEdgeSpec,
    progress: f64,
    scale: f32,
    surface_size: (f32, f32),
) -> Option<RevealEdgePixels> {
    if spec.length < 2. * REVEAL_EDGE_MIN_REVEALED || progress >= 1. {
        return None;
    }
    let body = reveal.sample(progress).body;
    let now = match (spec.vertical, spec.far) {
        (true, true) => body.y + body.h,
        (true, false) => body.y,
        (false, true) => body.x + body.w,
        (false, false) => body.x,
    };
    let revealed = spec.length - (now - spec.full_lead).abs();
    if revealed < REVEAL_EDGE_MIN_REVEALED {
        return None;
    }

    let (open, crop) = reveal.rasterize(1., scale, surface_size)?;
    let origin = if spec.vertical { crop.y } else { crop.x };
    let extent = if spec.vertical {
        open.height()
    } else {
        open.width()
    };
    let ghost_progress = ((REVEAL_EDGE_MIN_REVEALED + 24.) / spec.length).clamp(0.5, 0.9);
    let mut ghost_descriptor = reveal.clone();
    ghost_descriptor.animated_from = spec.closed_body;
    ghost_descriptor.animated_to = spec.full_body;
    let ghost = ghost_descriptor.sample(f64::from(ghost_progress)).body;
    let ghost_lead = match (spec.vertical, spec.far) {
        (true, true) => ghost.y + ghost.h,
        (true, false) => ghost.y,
        (false, true) => ghost.x + ghost.w,
        (false, false) => ghost.x,
    };
    let (low, high) = if spec.far {
        (
            ghost_lead - REVEAL_EDGE_INSIDE,
            ghost_lead + REVEAL_EDGE_OUTSIDE,
        )
    } else {
        (
            ghost_lead - REVEAL_EDGE_OUTSIDE,
            ghost_lead + REVEAL_EDGE_INSIDE,
        )
    };
    let first = (((low - origin) * scale).floor() as i64).max(0);
    let last = (((high - origin) * scale).ceil() as i64).min(i64::from(extent));
    if last - first < 4 {
        return None;
    }
    let ghost_crop = if spec.vertical {
        RevealRect::new(
            crop.x,
            origin + first as f32 / scale,
            crop.w,
            (last - first) as f32 / scale,
        )
    } else {
        RevealRect::new(
            origin + first as f32 / scale,
            crop.y,
            (last - first) as f32 / scale,
            crop.h,
        )
    };
    let (band, _) =
        reveal.rasterize_in_crop(f64::from(ghost_progress), scale, surface_size, ghost_crop)?;
    let band_extent = if spec.vertical {
        band.height()
    } else {
        band.width()
    };
    let last = first + i64::from(band_extent);
    let full_lead = spec.full_lead;
    let shift = ((((full_lead - ghost_lead) * scale).round() as i64).max(-first));
    if !reveal_edge_template_matches(&band, &open, first, last, shift, spec.vertical, spec.far) {
        return None;
    }
    Some(RevealEdgePixels {
        open,
        band,
        crop,
        vertical: spec.vertical,
        far: spec.far,
        origin,
        lead: (ghost_lead - origin) * scale,
        first,
        band_extent,
        total_extent: extent,
    })
}

impl<R: NiriRenderer> MotionPlaneRenderElement<R> {
    fn new(
        inner: MotionPlaneInner<R>,
        id: Id,
        damage: crate::render_helpers::damage::ExtraDamage,
    ) -> Self {
        Self { inner, id, damage }
    }
}

impl<R: NiriRenderer> Element for MotionPlaneRenderElement<R> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        self.damage.current_commit()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, smithay::utils::Physical> {
        self.inner.geometry(scale)
    }

    fn src(&self) -> Rectangle<f64, smithay::utils::Buffer> {
        self.inner.src()
    }

    fn transform(&self) -> smithay::utils::Transform {
        self.inner.transform()
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, smithay::utils::Physical> {
        self.damage.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, smithay::utils::Physical> {
        self.inner.opaque_regions(scale)
    }

    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }

    fn kind(&self) -> Kind {
        Kind::Unspecified
    }

    fn is_framebuffer_effect(&self) -> bool {
        false
    }
}

impl<R: NiriRenderer> RenderElement<R> for MotionPlaneRenderElement<R> {
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, smithay::utils::Buffer>,
        dst: Rectangle<i32, smithay::utils::Physical>,
        damage: &[Rectangle<i32, smithay::utils::Physical>],
        opaque_regions: &[Rectangle<i32, smithay::utils::Physical>],
        cache: Option<&UserDataMap>,
    ) -> Result<(), R::Error> {
        self.inner
            .draw(frame, src, dst, damage, opaque_regions, cache)
    }

    fn underlying_storage(&self, renderer: &mut R) -> Option<UnderlyingStorage<'_>> {
        self.inner.underlying_storage(renderer)
    }
}

impl MappedLayer {
    pub fn new(
        surface: LayerSurface,
        pre_commit_hook: HookId,
        rules: ResolvedLayerRules,
        view_size: Size<f64, Logical>,
        scale: f64,
        clock: Clock,
        config: &Config,
    ) -> Self {
        let mut shadow_config = config.layout.shadow;
        // Shadows for layer surfaces need to be explicitly enabled.
        shadow_config.on = false;
        shadow_config.merge_with(&rules.shadow);

        Self {
            surface,
            pre_commit_hook,
            rules,
            recompute_rules_on_commit: false,
            block_out_buffer: SolidColorBuffer::new((0., 0.), [0., 0., 0., 1.]),
            view_size,
            scale,
            shadow: Shadow::new(shadow_config),
            blur_config: config.blur,
            clock,
            motion: None,
            promotion_motion: None,
            reveal_texture_cache: RefCell::new(Vec::new()),
            reveal_edge_cache: RefCell::new(None),
            reveal_edge_rejected: RefCell::new(None),
        }
    }

    pub fn update_config(&mut self, config: &Config) {
        let mut shadow_config = config.layout.shadow;
        // Shadows for layer surfaces need to be explicitly enabled.
        shadow_config.on = false;
        shadow_config.merge_with(&self.rules.shadow);
        self.shadow.update_config(shadow_config);

        self.blur_config = config.blur;
    }

    pub fn update_shaders(&mut self) {
        self.shadow.update_shaders();
    }

    pub fn update_sizes(&mut self, view_size: Size<f64, Logical>, scale: f64) {
        if self.view_size != view_size || self.scale != scale {
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            self.reveal_texture_cache.borrow_mut().clear();
        }
        self.view_size = view_size;
        self.scale = scale;
    }

    pub fn update_render_elements(&mut self, size: Size<f64, Logical>) {
        // Round to physical pixels.
        let size = size
            .to_physical_precise_round(self.scale)
            .to_logical(self.scale);

        self.block_out_buffer.resize(size);

        let radius = self.rules.geometry_corner_radius.unwrap_or_default();
        // FIXME: is_active based on keyboard focus?
        self.shadow
            .update_render_elements(size, true, radius, self.scale, 1.);
    }

    pub fn are_animations_ongoing(&self) -> bool {
        if self
            .promotion_motion
            .as_ref()
            .is_some_and(|motion| !motion.is_complete(self.clock.now_unadjusted()))
        {
            return true;
        }
        self.rules.baba_is_float
            || self
                .motion
                .as_ref()
                .is_some_and(|motion| motion.lock().unwrap().active.is_some())
    }

    pub fn set_motion_runtime(&mut self, motion: Arc<Mutex<LayerMotionRuntime>>) {
        self.reveal_edge_cache.borrow_mut().take();
        self.reveal_edge_rejected.borrow_mut().take();
        self.reveal_texture_cache.borrow_mut().clear();
        self.motion = Some(motion);
    }

    pub fn motion_poses(&self) -> Option<Vec<(u32, MotionPose)>> {
        self.motion_snapshot().map(|(_, poses)| poses)
    }

    fn motion_snapshot(&self) -> Option<LayerMotionSnapshot> {
        let motion = self.motion.as_ref()?;
        let runtime = motion.lock().unwrap();
        if runtime.planes.is_empty() {
            self.reveal_texture_cache.borrow_mut().clear();
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            return None;
        }
        let poses = runtime
            .active
            .as_ref()
            .map(|active| active.sample(self.clock.now_unadjusted()))
            .unwrap_or_else(|| runtime.current.clone());
        Some((runtime.planes.clone(), poses))
    }

    fn reveal_snapshot(&self) -> Option<(RevealDescriptor, f64, f64)> {
        let motion = self.motion.as_ref()?;
        let runtime = motion.lock().unwrap();
        let reveal = runtime.reveal.clone()?;
        let (progress, opacity) = if let Some(active) = runtime.active.as_ref() {
            let progress = active.progress(self.clock.now_unadjusted());
            let opacity = active
                .sample(self.clock.now_unadjusted())
                .into_iter()
                .find(|(id, _)| *id == reveal.opacity_plane)
                .map_or(1., |(_, pose)| pose.opacity);
            (progress, opacity)
        } else {
            let opacity = runtime
                .current
                .iter()
                .find(|(id, _)| *id == reveal.opacity_plane)
                .map_or(1., |(_, pose)| pose.opacity);
            (1., opacity)
        };
        Some((reveal, progress, opacity))
    }

    fn render_edge_template<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        reveal: &RevealDescriptor,
        progress: f64,
        opacity: f32,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        surface_size: (f32, f32),
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) -> bool {
        let Some(spec) = reveal_edge_spec(reveal) else {
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            return false;
        };
        let current_body = reveal.sample(progress).body;
        let now = match (spec.vertical, spec.far) {
            (true, true) => current_body.y + current_body.h,
            (true, false) => current_body.y,
            (false, true) => current_body.x + current_body.w,
            (false, false) => current_body.x,
        };
        let revealed = spec.length - (now - spec.full_lead).abs();
        let key = RevealEdgeCacheKey {
            descriptor: reveal.clone(),
            scale: (scale.x as f32).to_bits(),
            surface_width: surface_size.0.to_bits(),
            surface_height: surface_size.1.to_bits(),
            location_x: location.x.to_bits(),
            location_y: location.y.to_bits(),
        };
        {
            let mut rejected = self.reveal_edge_rejected.borrow_mut();
            if rejected.as_ref().is_some_and(|rejected| rejected != &key) {
                rejected.take();
            }
            if rejected.as_ref() == Some(&key) {
                return false;
            }
        }
        if progress < 1.
            && (spec.length < 2. * REVEAL_EDGE_MIN_REVEALED || revealed < REVEAL_EDGE_MIN_REVEALED)
        {
            return false;
        }
        let cached = {
            let mut cache = self.reveal_edge_cache.borrow_mut();
            if cache.as_ref().is_some_and(|entry| entry.key != key) {
                cache.take();
            }
            cache.as_ref().and_then(|entry| {
                let open = entry
                    .open
                    .downcast_ref::<TextureBuffer<R::NiriTextureId>>()?;
                let band = entry
                    .band
                    .downcast_ref::<TextureBuffer<R::NiriTextureId>>()?;
                (open.belongs_to_renderer(renderer) && band.belongs_to_renderer(renderer)).then(
                    || {
                        (
                            open.clone(),
                            band.clone(),
                            entry.crop,
                            entry.vertical,
                            entry.far,
                            entry.origin,
                            entry.lead,
                            entry.first,
                            entry.band_extent,
                            entry.total_extent,
                        )
                    },
                )
            })
        };
        let cached = if let Some(cached) = cached {
            Some(cached)
        } else {
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            if progress >= 1. {
                return false;
            }
            let Some(pixels) =
                build_reveal_edge_pixels(reveal, &spec, progress, scale.x as f32, surface_size)
            else {
                *self.reveal_edge_rejected.borrow_mut() = Some(key);
                return false;
            };
            if pixels.open.data().len() + pixels.band.data().len() > REVEAL_EDGE_CACHE_LIMIT {
                *self.reveal_edge_rejected.borrow_mut() = Some(key);
                return false;
            }
            let open_size = Size::from((pixels.open.width() as i32, pixels.open.height() as i32));
            let band_size = Size::from((pixels.band.width() as i32, pixels.band.height() as i32));
            let Ok(open) = TextureBuffer::from_memory(
                renderer,
                pixels.open.data(),
                Fourcc::Abgr8888,
                open_size,
                false,
                scale,
                Transform::Normal,
                Vec::new(),
            ) else {
                *self.reveal_edge_rejected.borrow_mut() = Some(key);
                return false;
            };
            let Ok(band) = TextureBuffer::from_memory(
                renderer,
                pixels.band.data(),
                Fourcc::Abgr8888,
                band_size,
                false,
                scale,
                Transform::Normal,
                Vec::new(),
            ) else {
                *self.reveal_edge_rejected.borrow_mut() = Some(key);
                return false;
            };
            let cached = (
                open.clone(),
                band.clone(),
                pixels.crop,
                pixels.vertical,
                pixels.far,
                pixels.origin,
                pixels.lead,
                pixels.first,
                pixels.band_extent,
                pixels.total_extent,
            );
            *self.reveal_edge_cache.borrow_mut() = Some(RevealEdgeTextureCache {
                key,
                open: Box::new(open),
                band: Box::new(band),
                crop: pixels.crop,
                vertical: pixels.vertical,
                far: pixels.far,
                origin: pixels.origin,
                lead: pixels.lead,
                first: pixels.first,
                band_extent: pixels.band_extent,
                total_extent: pixels.total_extent,
            });
            self.reveal_edge_rejected.borrow_mut().take();
            self.reveal_texture_cache.borrow_mut().clear();
            Some(cached)
        };
        let Some((open, band, crop, vertical, far, origin, lead, first, band_extent, total_extent)) =
            cached
        else {
            return false;
        };
        let shift = ((now - origin) * scale.x as f32 - lead).round() as i64;
        let start = first + shift;
        if start < 0 || start + i64::from(band_extent) > i64::from(total_extent) {
            return false;
        }
        let open_element = TextureRenderElement::from_texture_buffer(
            open,
            location + Point::from((crop.x as f64, crop.y as f64)),
            opacity,
            None,
            None,
            Kind::Unspecified,
        );
        if progress < 1. {
            let edge_start = origin + start as f32 / scale.x as f32;
            let edge_end = origin + (start + i64::from(band_extent)) as f32 / scale.x as f32;
            let open_clip = if vertical {
                if far {
                    RevealRect::new(crop.x, crop.y, crop.w, edge_start - crop.y)
                } else {
                    RevealRect::new(crop.x, edge_end, crop.w, crop.y + crop.h - edge_end)
                }
            } else if far {
                RevealRect::new(crop.x, crop.y, edge_start - crop.x, crop.h)
            } else {
                RevealRect::new(edge_end, crop.y, crop.x + crop.w - edge_end, crop.h)
            };
            if open_clip.w > 0. && open_clip.h > 0. {
                let clip = Rectangle::new(
                    location + Point::from((open_clip.x as f64, open_clip.y as f64)),
                    Size::from((open_clip.w as f64, open_clip.h as f64)),
                );
                if let Some(element) = CropRenderElement::from_element(
                    open_element.clone(),
                    scale,
                    to_physical_cover(clip, scale),
                ) {
                    push(element.into());
                }
            }
            let band_location = if vertical {
                location + Point::from((crop.x as f64, origin as f64 + start as f64 / scale.x))
            } else {
                location + Point::from((origin as f64 + start as f64 / scale.x, crop.y as f64))
            };
            let band_element = TextureRenderElement::from_texture_buffer(
                band,
                band_location,
                opacity,
                None,
                None,
                Kind::Unspecified,
            );
            push(band_element.into());
        } else {
            push(open_element.into());
        }
        true
    }

    fn render_reveal<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        let Some((reveal, progress, opacity)) = self.reveal_snapshot() else {
            self.reveal_texture_cache.borrow_mut().clear();
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            return;
        };
        if !reveal.visible {
            self.reveal_texture_cache.borrow_mut().clear();
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            return;
        }
        let Some(surface_geo) = with_states(self.surface.wl_surface(), crate::utils::surface_geo)
        else {
            self.reveal_texture_cache.borrow_mut().clear();
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            return;
        };
        if reveal.fill[3] == 0
            && reveal
                .rim
                .as_ref()
                .is_none_or(|rim| rim.width == 0. || rim.opacity == 0.)
            && reveal.shadow.is_none()
        {
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            self.reveal_texture_cache.borrow_mut().clear();
            return;
        }
        if reveal.static_rects.is_empty()
            && reveal.holes.is_empty()
            && reveal.corner_mode == super::reveal::CornerMode::Circular
            && reveal.rim.is_none()
        {
            self.reveal_edge_cache.borrow_mut().take();
            self.reveal_edge_rejected.borrow_mut().take();
            let body = reveal.sample(progress).body;
            let crop = reveal.crop;
            let size = Size::from((crop.w as f64, crop.h as f64));
            if size.w <= 0. || size.h <= 0. {
                return;
            }
            let mut radii = reveal.sample(progress).radii;
            if body.x <= reveal.output_bounds.x && reveal.square_edges & 1 != 0 {
                radii[0] = 0.;
                radii[3] = 0.;
            }
            if body.y <= reveal.output_bounds.y && reveal.square_edges & 2 != 0 {
                radii[0] = 0.;
                radii[1] = 0.;
            }
            if body.x + body.w >= reveal.output_bounds.x + reveal.output_bounds.w
                && reveal.square_edges & 4 != 0
            {
                radii[1] = 0.;
                radii[2] = 0.;
            }
            if body.y + body.h >= reveal.output_bounds.y + reveal.output_bounds.h
                && reveal.square_edges & 8 != 0
            {
                radii[2] = 0.;
                radii[3] = 0.;
            }
            let maximum_radius = (body.w.min(body.h) / 2.).max(0.);
            radii
                .iter_mut()
                .for_each(|radius| *radius = radius.min(maximum_radius));
            let radius = CornerRadius {
                top_left: radii[0],
                top_right: radii[1],
                bottom_right: radii[2],
                bottom_left: radii[3],
            };
            let area = Rectangle::from_size(size);
            let geometry = Rectangle::new(
                Point::from(((body.x - crop.x) as f64, (body.y - crop.y) as f64)),
                Size::from((body.w as f64, body.h as f64)),
            );
            let color = Color::from_rgba8_unpremul(
                reveal.fill[0],
                reveal.fill[1],
                reveal.fill[2],
                reveal.fill[3],
            );
            let border = BorderRenderElement::new(
                size,
                area,
                GradientInterpolation::default(),
                color,
                color,
                0.,
                geometry,
                0.,
                radius,
                scale.x as f32,
                alpha * opacity as f32,
            )
            .with_location(location + Point::from((crop.x as f64, crop.y as f64)));
            push(border.into());
            if let Some(shadow) = self.create_reveal_shadow(&reveal, progress, location, scale) {
                push(shadow.with_alpha(alpha * opacity as f32).into());
            }
            self.reveal_texture_cache.borrow_mut().clear();
            return;
        }
        if self.render_edge_template(
            renderer,
            &reveal,
            progress,
            alpha * opacity as f32,
            location,
            scale,
            (surface_geo.size.w as f32, surface_geo.size.h as f32),
            push,
        ) {
            self.reveal_texture_cache.borrow_mut().clear();
            return;
        }
        let key = RevealTextureCacheKey {
            descriptor: reveal.clone(),
            progress: progress.to_bits(),
            scale: (scale.x as f32).to_bits(),
            surface_width: (surface_geo.size.w as f32).to_bits(),
            surface_height: (surface_geo.size.h as f32).to_bits(),
            location_x: location.x.to_bits(),
            location_y: location.y.to_bits(),
        };
        let cached = self.reveal_texture_cache.borrow().iter().find_map(|entry| {
            if entry.key != key {
                return None;
            }
            let texture = entry
                .texture
                .downcast_ref::<TextureBuffer<R::NiriTextureId>>()?;
            texture
                .belongs_to_renderer(renderer)
                .then(|| (texture.clone(), entry.shadow.clone(), entry.crop))
        });
        let (buffer, crop, cached_shadow) = if let Some((buffer, shadow, crop)) = cached {
            (buffer, crop, shadow)
        } else {
            let Some((pixmap, crop)) = reveal.rasterize(
                progress,
                scale.x as f32,
                (surface_geo.size.w as f32, surface_geo.size.h as f32),
            ) else {
                return;
            };
            let size = (pixmap.width(), pixmap.height());
            let Ok(buffer) = TextureBuffer::from_memory(
                renderer,
                pixmap.data(),
                Fourcc::Abgr8888,
                Size::from((size.0 as i32, size.1 as i32)),
                false,
                scale,
                Transform::Normal,
                Vec::new(),
            ) else {
                return;
            };
            let shadow = self.create_reveal_shadow(&reveal, progress, location, scale);
            let mut cache = self.reveal_texture_cache.borrow_mut();
            cache.retain(|entry| entry.key != key);
            cache.push(RevealTextureCache {
                key,
                texture: Box::new(buffer.clone()),
                shadow: shadow.clone(),
                crop,
            });
            if cache.len() > 1 {
                cache.remove(0);
            }
            (buffer, crop, shadow)
        };
        let element = TextureRenderElement::from_texture_buffer(
            buffer,
            location + Point::from((crop.x as f64, crop.y as f64)),
            alpha * opacity as f32,
            None,
            None,
            Kind::Unspecified,
        );
        push(element.into());
        if let Some(shadow) = cached_shadow {
            push(shadow.with_alpha(alpha * opacity as f32).into());
        }
    }

    fn create_reveal_shadow(
        &self,
        reveal: &RevealDescriptor,
        progress: f64,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
    ) -> Option<ShadowRenderElement> {
        let shadow = reveal.shadow.as_ref()?;
        if !reveal.static_rects.is_empty()
            || !reveal.holes.is_empty()
            || reveal.corner_mode != super::reveal::CornerMode::Circular
        {
            return None;
        }
        let body = reveal.sample(progress).body;
        // The token is already Gaussian standard deviation, as used by
        // Bead's SVG feGaussianBlur and Niri's rounded-box shadow shader.
        let sigma = shadow.blur_radius;
        let scale = scale.x as f32;
        let ceil = |value: f32| (value * scale).ceil() / scale;
        let extent = ceil(sigma * 3.);
        let spread = ceil(shadow.spread.abs()).copysign(shadow.spread);
        let offset: Point<f32, Logical> =
            Point::from((ceil(shadow.offset.0), ceil(shadow.offset.1)));
        let positive_spread = spread.max(0.);
        let negative_spread = (-spread).max(0.);
        let size = Size::from((
            (body.w + positive_spread * 2. + extent * 2.) as f64,
            (body.h + positive_spread * 2. + extent * 2.) as f64,
        ));
        let shape_width = body.w + spread * 2.;
        let shape_height = body.h + spread * 2.;
        let shape_size = Size::from((shape_width as f64, shape_height as f64));
        if size.w <= 0. || size.h <= 0. || shape_width <= 0. || shape_height <= 0. {
            return None;
        }
        let shape_loc = Point::from((
            (extent + negative_spread) as f64,
            (extent + negative_spread) as f64,
        ));
        let window_loc = Point::from((
            (extent + positive_spread - offset.x) as f64,
            (extent + positive_spread - offset.y) as f64,
        ));
        let sampled = reveal.sample(progress);
        let window_radius = CornerRadius {
            top_left: sampled.radii[0],
            top_right: sampled.radii[1],
            bottom_right: sampled.radii[2],
            bottom_left: sampled.radii[3],
        }
        .fit_to(body.w, body.h);
        let shadow_radius = window_radius
            .expanded_by(spread)
            .fit_to(shape_width, shape_height);
        Some(
            ShadowRenderElement::new(
                size,
                Rectangle::new(shape_loc, shape_size),
                Color::from_rgba8_unpremul(
                    shadow.color[0],
                    shadow.color[1],
                    shadow.color[2],
                    shadow.color[3],
                ),
                sigma,
                shadow_radius,
                scale,
                Rectangle::new(window_loc, Size::from((body.w as f64, body.h as f64))),
                window_radius,
                1.,
            )
            .with_location(
                location
                    + Point::from((
                        (body.x - extent - positive_spread + offset.x) as f64,
                        (body.y - extent - positive_spread + offset.y) as f64,
                    )),
            ),
        )
    }

    /// `None` means ordinary layer hit testing; `Some(None)` means the animated
    /// atlas does not cover this point; `Some(Some(point))` maps into atlas input.
    pub fn motion_input_point(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<Option<Point<f64, Logical>>> {
        if let Some(motion) = &self.motion {
            let runtime = motion.lock().unwrap();
            if let Some(reveal) = &runtime.reveal {
                let progress = runtime
                    .active
                    .as_ref()
                    .map_or(1., |active| active.progress(self.clock.now_unadjusted()));
                if !reveal.contains((point.x, point.y), progress) {
                    return Some(None);
                }
            }
        }
        let (planes, poses) = self.motion_snapshot()?;
        let pose_for = |id| {
            poses
                .iter()
                .find(|(plane, _)| *plane == id)
                .map(|(_, pose)| *pose)
        };
        let hit = planes.iter().rev().find_map(|plane| {
            let pose = pose_for(plane.id)?;
            pose.map_input(point, plane.source)
        });
        Some(hit)
    }

    pub fn surface_under_motion(
        &self,
        point: Point<f64, Logical>,
        surface_type: WindowSurfaceType,
    ) -> Option<
        Option<(
            smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
            Point<f64, Logical>,
        )>,
    > {
        let source_point = self.motion_input_point(point)?;
        let Some(source_point) = source_point else {
            return Some(None);
        };
        Some(
            self.surface
                .surface_under(source_point, surface_type)
                .map(|(surface, location)| {
                    (surface.clone(), location.to_f64() + (point - source_point))
                }),
        )
    }

    pub fn surface(&self) -> &LayerSurface {
        &self.surface
    }

    pub fn rules(&self) -> &ResolvedLayerRules {
        &self.rules
    }

    /// Recomputes the resolved layer rules and returns whether they changed.
    pub fn recompute_layer_rules(&mut self, rules: &[LayerRule], is_at_startup: bool) -> bool {
        let new_rules = ResolvedLayerRules::compute(rules, &self.surface, is_at_startup);
        if new_rules == self.rules {
            return false;
        }

        self.rules = new_rules;
        true
    }

    pub fn set_recompute_rules_on_commit(&mut self) {
        self.recompute_rules_on_commit = true;
    }

    pub fn take_recompute_rules_on_commit(&mut self) -> bool {
        std::mem::take(&mut self.recompute_rules_on_commit)
    }

    pub fn place_within_backdrop(&self) -> bool {
        if !self.rules.place_within_backdrop {
            return false;
        }

        if self.surface.layer() != Layer::Background {
            return false;
        }

        let state = self.surface.cached_state();
        state.exclusive_zone == ExclusiveZone::DontCare
    }

    pub fn bob_offset(&self) -> Point<f64, Logical> {
        let offset = self
            .promotion_motion
            .as_ref()
            .map(|motion| motion.sample(self.clock.now_unadjusted()).destination.loc)
            .unwrap_or_default();
        if !self.rules.baba_is_float {
            return offset;
        }

        let y = baba_is_float_offset(self.clock.now(), self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        offset + Point::from((0., y))
    }

    /// Return the retained surface to its dock using the layer-motion sampler.
    /// Input and rendering both consume bob_offset(), including at scale 1.5.
    pub fn start_promotion_return(
        &mut self,
        serial: u32,
        from: Point<f64, Logical>,
        start: std::time::Duration,
    ) {
        let pose = |at| MotionPose {
            destination: Rectangle::new(at, Size::from((1., 1.))),
            clip: Rectangle::from_size(Size::from((1., 1.))),
            opacity: 1.,
        };
        self.promotion_motion = LayerMotion::new(
            serial,
            pose(from),
            pose(Point::from((0., 0.))),
            start,
            std::time::Duration::from_millis(180),
            MotionCurve::EaseOutCubic,
            OpacityMode::Linear,
        );
    }

    pub fn clear_promotion_return(&mut self) {
        self.promotion_motion = None;
    }

    pub fn render_normal<R: NiriRenderer>(
        &self,
        ctx: RenderCtx<R>,
        ns: Option<usize>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        self.render_with_policy(ctx, ns, location, xray_pos, false, push);
    }

    /// Promotion uses the same surface tree and motion planes, with compositor
    /// policy that supplies no layer opacity, bob, blur, or shadow decoration.
    pub fn render_promoted<R: NiriRenderer>(
        &self,
        ctx: RenderCtx<R>,
        location: Point<f64, Logical>,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        self.render_with_policy(ctx, None, location, XrayPos::default(), true, push);
    }

    fn render_with_policy<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        ns: Option<usize>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        promoted: bool,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);
        let alpha = if promoted {
            1.
        } else {
            self.rules.opacity.unwrap_or(1.).clamp(0., 1.)
        };

        let bob_offset = if promoted {
            Point::from((0., 0.))
        } else {
            self.bob_offset()
        };
        let location = location + bob_offset;
        let xray_pos = xray_pos.offset(bob_offset);

        let surface = self.surface.wl_surface();

        let should_block_out = ctx.target.should_block_out(self.rules.block_out_from);
        if should_block_out {
            // Round to physical pixels.
            let location = location.to_physical_precise_round(scale).to_logical(scale);

            // FIXME: take geometry-corner-radius into account.
            let elem = SolidColorRenderElement::from_buffer(
                &self.block_out_buffer,
                location,
                alpha,
                Kind::Unspecified,
            );
            push(elem.into());
        } else if let Some((planes, poses)) = self.motion_snapshot() {
            self.render_motion_planes(
                ctx.renderer,
                surface,
                location,
                scale,
                alpha,
                &planes,
                &poses,
                push,
            );
            self.render_reveal(ctx.renderer, location, scale, alpha, push);
        } else {
            // Layer surfaces don't have extra geometry like windows.
            let buf_pos = location;

            push_elements_from_surface_tree(
                ctx.renderer,
                surface,
                buf_pos.to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
                &mut |elem| push(elem.into()),
            );
        }

        if promoted {
            return;
        }
        let location = location.to_physical_precise_round(scale).to_logical(scale);
        self.shadow
            .render(ctx.renderer, location, &mut |elem| push(elem.into()));

        let geometry = Rectangle::new(location, self.block_out_buffer.size());
        let surface_off = Point::new(0., 0.); // No geometry on layer surfaces.
        let surface_anim_scale = Scale::from(1.);
        let radius = self.rules.geometry_corner_radius.unwrap_or_default();
        background_effect::render_for_tile(
            ctx.as_gles(),
            ns,
            geometry,
            self.scale,
            false,
            surface,
            surface_off,
            surface_anim_scale,
            self.blur_config,
            radius,
            self.rules.background_effect,
            should_block_out,
            xray_pos,
            &mut |elem| push(elem.into()),
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn render_motion_planes<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        surface: &smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
        location: Point<f64, Logical>,
        scale: Scale<f64>,
        alpha: f32,
        planes: &[MotionPlane],
        poses: &[(u32, MotionPose)],
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        let root_location = location.to_physical_precise_round(scale);
        let root_id = Id::from_wayland_resource(surface);
        let reveal_state = self.reveal_snapshot();
        let animated_content_clip = reveal_state.as_ref().and_then(|(reveal, progress, _)| {
            reveal
                .visible
                .then(|| reveal.sample(*progress).content_clip)
                .flatten()
                .map(|clip| {
                    Rectangle::new(
                        Point::from((clip.x as f64, clip.y as f64)),
                        Size::from((clip.w as f64, clip.h as f64)),
                    )
                })
        });
        for plane in planes {
            let Some((_, pose)) = poses.iter().find(|(id, _)| *id == plane.id) else {
                continue;
            };
            if pose.destination.size.w <= 0.
                || pose.destination.size.h <= 0.
                || plane.source.size.w <= 0.
                || plane.source.size.h <= 0.
            {
                continue;
            }
            let stretch = Scale::from((
                pose.destination.size.w / plane.source.size.w,
                pose.destination.size.h / plane.source.size.h,
            ));
            let destination_logical =
                Rectangle::new(location + pose.destination.loc, pose.destination.size);
            let destination_physical = destination_logical.to_physical_precise_round(scale);
            let mut clip_logical = Rectangle::new(location + pose.clip.loc, pose.clip.size);
            if let Some(reveal_clip) = animated_content_clip {
                let Some(intersection) = clip_logical
                    .intersection(Rectangle::new(location + reveal_clip.loc, reveal_clip.size))
                else {
                    continue;
                };
                clip_logical = intersection;
            }
            let clip_physical = to_physical_cover(clip_logical, scale);
            let Some(visible_physical) = destination_physical.intersection(clip_physical) else {
                continue;
            };
            let Some(visible_logical) = destination_logical.intersection(clip_logical) else {
                continue;
            };
            let source_location = Point::from((
                (root_location.x as f64 + plane.source.loc.x * self.scale * stretch.x).round()
                    as i32,
                (root_location.y as f64 + plane.source.loc.y * self.scale * stretch.y).round()
                    as i32,
            ));
            let source_crop = Rectangle::new(source_location, destination_physical.size);
            push_elements_from_surface_tree(
                renderer,
                surface,
                root_location,
                scale,
                alpha * pose.opacity as f32,
                Kind::Unspecified,
                &mut |elem| {
                    if elem.id() != &root_id {
                        return;
                    }
                    let scaled = RescaleRenderElement::from_element(elem, root_location, stretch);
                    let Some(source_crop) =
                        CropRenderElement::from_element(scaled, scale, source_crop)
                    else {
                        return;
                    };
                    let relocation = destination_physical.loc - source_crop.geometry(scale).loc;
                    let relocated = RelocateRenderElement::from_element(
                        source_crop,
                        relocation,
                        Relocate::Relative,
                    );
                    let Some(inner) =
                        CropRenderElement::from_element(relocated, scale, visible_physical)
                    else {
                        return;
                    };
                    let damage = plane.damage.render(visible_logical);
                    let element =
                        MotionPlaneRenderElement::new(inner, plane.render_id.clone(), damage);
                    push(element.into());
                },
            );
        }
    }

    pub fn render_popups<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        ns: Option<usize>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        if ctx.target.should_block_out(self.rules.block_out_from) {
            return;
        }

        let scale = Scale::from(self.scale);
        let alpha = self.rules.opacity.unwrap_or(1.).clamp(0., 1.);

        let bob_offset = self.bob_offset();
        let location = location + bob_offset;
        let xray_pos = xray_pos.offset(bob_offset);

        let surface = self.surface.wl_surface();
        for (popup, offset) in PopupManager::popups_for_surface(surface) {
            let popup_rules = match popup {
                PopupKind::Xdg(_) => self.rules.popups,
                // IME popups aren't affected by rules for regular popups.
                PopupKind::InputMethod(_) => niri_config::ResolvedPopupsRules::default(),
            };
            let alpha = alpha * popup_rules.opacity.unwrap_or(1.).clamp(0., 1.);

            let surface = popup.wl_surface();
            let popup_geo = popup.geometry();
            let surface_loc = location + (offset - popup_geo.loc).to_f64();

            push_elements_from_surface_tree(
                ctx.renderer,
                surface,
                surface_loc.to_physical_precise_round(scale),
                scale,
                alpha,
                Kind::ScanoutCandidate,
                &mut |elem| push(elem.into()),
            );

            let geometry = Rectangle::new(location + offset.to_f64(), popup_geo.size.to_f64());
            let surface_off = popup_geo.loc.upscale(-1).to_f64();
            let surface_anim_scale = Scale::from(1.);
            let mut effect = popup_rules.background_effect;
            // Default xray to false for pop-ups since they're always on top of something.
            if effect.xray.is_none() {
                effect.xray = Some(false);
            }
            let xray_pos = xray_pos.offset(offset.to_f64());
            background_effect::render_for_tile(
                ctx.as_gles(),
                ns,
                geometry,
                self.scale,
                false,
                surface,
                surface_off,
                surface_anim_scale,
                self.blur_config,
                popup_rules.geometry_corner_radius.unwrap_or_default(),
                effect,
                false,
                xray_pos,
                &mut |elem| push(elem.into()),
            );
        }
    }
}

impl Drop for MappedLayer {
    fn drop(&mut self) {
        remove_pre_commit_hook(self.surface.wl_surface(), &self.pre_commit_hook);
    }
}
