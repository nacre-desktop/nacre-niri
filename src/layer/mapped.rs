use std::sync::{Arc, Mutex};

use niri_config::utils::MergeWith as _;
use niri_config::{Config, LayerRule};
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::backend::renderer::Renderer;
use smithay::utils::user_data::UserDataMap;
use smithay::desktop::{LayerSurface, PopupKind, PopupManager, WindowSurfaceType};
use smithay::utils::{Logical, Point, Rectangle, Scale, Size};
use smithay::wayland::compositor::{remove_pre_commit_hook, HookId};
use smithay::wayland::shell::wlr_layer::{ExclusiveZone, Layer};

use super::ResolvedLayerRules;
use super::motion::MotionPose;
use crate::animation::Clock;
use crate::layout::shadow::Shadow;
use crate::niri_render_elements;
use crate::protocols::layer_motion::{LayerMotionRuntime, MotionPlane};
use crate::render_helpers::background_effect::BackgroundEffectElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::{SolidColorBuffer, SolidColorRenderElement};
use crate::render_helpers::surface::push_elements_from_surface_tree;
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
}

niri_render_elements! {
    LayerSurfaceRenderElement<R> => {
        Wayland = WaylandSurfaceRenderElement<R>,
        MotionPlane = MotionPlaneRenderElement<R>,
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
        self.inner.draw(frame, src, dst, damage, opaque_regions, cache)
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
        self.rules.baba_is_float
            || self
                .motion
                .as_ref()
                .is_some_and(|motion| motion.lock().unwrap().active.is_some())
    }

    pub fn set_motion_runtime(&mut self, motion: Arc<Mutex<LayerMotionRuntime>>) {
        self.motion = Some(motion);
    }

    pub fn motion_poses(&self) -> Option<Vec<(u32, MotionPose)>> {
        self.motion_snapshot().map(|(_, poses)| poses)
    }

    fn motion_snapshot(&self) -> Option<(Vec<MotionPlane>, Vec<(u32, MotionPose)>)> {
        let motion = self.motion.as_ref()?;
        let runtime = motion.lock().unwrap();
        if runtime.planes.is_empty() {
            return None;
        }
        let poses = runtime
            .active
            .as_ref()
            .map(|active| active.sample(self.clock.now()))
            .unwrap_or_else(|| runtime.current.clone());
        Some((runtime.planes.clone(), poses))
    }

    /// `None` means ordinary layer hit testing; `Some(None)` means the animated
    /// atlas does not cover this point; `Some(Some(point))` maps into atlas input.
    pub fn motion_input_point(
        &self,
        point: Point<f64, Logical>,
    ) -> Option<Option<Point<f64, Logical>>> {
        let (planes, poses) = self.motion_snapshot()?;
        let pose_for = |id| poses.iter().find(|(plane, _)| *plane == id).map(|(_, pose)| *pose);
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
    ) -> Option<Option<(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface, Point<f64, Logical>)>> {
        let source_point = self.motion_input_point(point)?;
        let Some(source_point) = source_point else {
            return Some(None);
        };
        Some(
            self.surface
                .surface_under(source_point, surface_type)
                .map(|(surface, location)| {
                    (
                        surface.clone(),
                        location.to_f64() + (point - source_point),
                    )
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
        if state.exclusive_zone != ExclusiveZone::DontCare {
            return false;
        }

        true
    }

    pub fn bob_offset(&self) -> Point<f64, Logical> {
        if !self.rules.baba_is_float {
            return Point::from((0., 0.));
        }

        let y = baba_is_float_offset(self.clock.now(), self.view_size.h);
        let y = round_logical_in_physical(self.scale, y);
        Point::from((0., y))
    }

    pub fn render_normal<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        ns: Option<usize>,
        location: Point<f64, Logical>,
        xray_pos: XrayPos,
        push: &mut dyn FnMut(LayerSurfaceRenderElement<R>),
    ) {
        let scale = Scale::from(self.scale);
        let alpha = self.rules.opacity.unwrap_or(1.).clamp(0., 1.);

        let bob_offset = self.bob_offset();
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
            let clip_logical = Rectangle::new(location + pose.clip.loc, pose.clip.size);
            let clip_physical = clip_logical.to_physical_precise_round(scale);
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
                    push(
                        MotionPlaneRenderElement::new(
                            inner,
                            plane.render_id.clone(),
                            damage,
                        )
                        .into(),
                    );
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
