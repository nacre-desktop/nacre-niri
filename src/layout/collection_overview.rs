//! Shared collection-column geometry for rendering, pointer selection, and drag targets.

#[cfg(test)]
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::utils::{
    CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement,
};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::GlesTexture;
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, ContextId, Renderer as _};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use super::monitor::{InsertPosition, MonitorAddWindowTarget};
use super::insert_hint_element::{InsertHintElement, InsertHintRenderElement};
use super::workspace::{Workspace, WorkspaceAddWindowTarget, WorkspaceId, WorkspaceRenderElement};
use super::{ActivateWindow, HitType, InteractiveMoveData, Layout, LayoutElement};
use crate::niri_render_elements;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::shadow::ShadowRenderElement;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::{render_to_texture, RenderCtx, RenderTarget};
use crate::utils::output_size;

const SCRATCHPAD_COLUMN_ID: u64 = u64::MAX;

#[derive(Debug, Default)]
pub(super) struct Overview {
    labels: RefCell<HashMap<(u64, String, u64), TextureBuffer<GlesTexture>>>,
    preview_geometry: RefCell<HashMap<Vec<u64>, Vec<PreviewGeometry>>>,
    prepared_revisions: RefCell<HashMap<WorkspaceId, u64>>,
    textures: RefCell<Vec<PreviewTexture>>,
    drop_hints: RefCell<HashMap<Output, InsertHintElement>>,
    #[cfg(test)]
    preparations: Cell<u64>,
    #[cfg(test)]
    texture_builds: Cell<u64>,
}

#[derive(Debug, Clone, Copy)]
struct PreviewGeometry {
    collection: u64,
    workspace: WorkspaceId,
    is_scratchpad: bool,
    geometry: Rectangle<f64, Logical>,
    zoom: f64,
}

niri_render_elements! {
    CollectionOverviewRenderElement<R> => {
        Workspace = CropRenderElement<RelocateRenderElement<RescaleRenderElement<WorkspaceRenderElement<R>>>>,
        Background = CropRenderElement<RelocateRenderElement<RescaleRenderElement<SolidColorRenderElement>>>,
        Shadow = CropRenderElement<RelocateRenderElement<RescaleRenderElement<ShadowRenderElement>>>,
        Label = PrimaryGpuTextureRenderElement,
        CachedPreview = CropRenderElement<RelocateRenderElement<RescaleRenderElement<PrimaryGpuTextureRenderElement>>>,
        InsertHint = CropRenderElement<InsertHintRenderElement>,
    }
}

niri_render_elements! {
    HiddenSnapshotRenderElement<R> => {
        Workspace = WorkspaceRenderElement<R>,
        Background = SolidColorRenderElement,
    }
}

/// The target and renderer are part of identity: output and capture privacy rules must never
/// share snapshots, nor may a texture from one GL context be used in another.
#[derive(Debug, PartialEq)]
struct TextureKey {
    output: Output,
    context: ContextId<GlesTexture>,
    target: RenderTarget,
    workspace: WorkspaceId,
    revision: u64,
    scale: f64,
    location: Point<f64, Logical>,
    zoom: f64,
    xray: Option<[(Id, CommitCounter); 2]>,
    xray_geometry: Option<XrayGeometry>,
}

#[derive(Debug, PartialEq)]
struct XrayGeometry {
    backdrop_color: Color32F,
    workspaces: Vec<(Rectangle<f64, Logical>, Color32F)>,
}

#[derive(Debug)]
struct PreviewTexture {
    key: TextureKey,
    buffer: TextureBuffer<GlesTexture>,
}

pub(super) struct Preview<'a, W: LayoutElement> {
    pub(super) id: u64,
    pub(super) workspace: &'a Workspace<W>,
    pub(super) geometry: Rectangle<f64, Logical>,
    pub(super) zoom: f64,
    pub(super) is_scratchpad: bool,
}

impl<W: LayoutElement> Layout<W> {
    fn scratchpad_preview(&self, output: &Output) -> Option<Preview<'_, W>> {
        if !self.overview_open || !self.scratchpad.has_windows() {
            return None;
        }
        let size = output_size(output);
        let width = (size.w * 0.46).min(720.).max(1.);
        let height = (size.h * 0.32).min(300.).max(1.);
        let bounds = Rectangle::new(
            (size.w - width - 16., size.h - height - 16.).into(),
            Size::from((width, height)),
        );
        let zoom = (width / self.scratchpad.view_size().w.max(1.))
            .min(height / self.scratchpad.view_size().h.max(1.));
        let rendered = Size::from((
            self.scratchpad.view_size().w * zoom,
            self.scratchpad.view_size().h * zoom,
        ));
        let geometry = Rectangle::new(
            bounds.loc + (bounds.size.to_point() - rendered.to_point()).downscale(2.),
            rendered,
        );
        Some(Preview {
            id: SCRATCHPAD_COLUMN_ID,
            workspace: &self.scratchpad,
            geometry,
            zoom,
            is_scratchpad: true,
        })
    }

    pub(super) fn scratchpad_overview_window_under(
        &self,
        output: &Output,
        pos: Point<f64, Logical>,
    ) -> Option<(&W, HitType)> {
        let preview = self.scratchpad_preview(output)?;
        if !preview.geometry.contains(pos) {
            return None;
        }
        let (window, hit) = preview
            .workspace
            .window_under((pos - preview.geometry.loc).downscale(preview.zoom))?;
        Some((window, hit.to_activate()))
    }

    pub fn render_scratchpad_overview<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output: &Output,
        _focus_ring: bool,
        push: &mut dyn FnMut(CollectionOverviewRenderElement<R>),
    ) {
        if self.collection_overview_is_open() {
            return;
        }
        let Some(preview) = self.scratchpad_preview(output) else {
            return;
        };
        let scale = output.current_scale().fractional_scale();
        let crop = Rectangle::from_size(output_size(output)).to_physical_precise_round(scale);
        let location = preview.geometry.loc.to_physical_precise_round(scale);
        if let Some(label) = self.collection_overview.label(
            ctx.renderer,
            SCRATCHPAD_COLUMN_ID,
            "Scratchpad",
            scale,
            (
                preview.geometry.loc.x,
                (preview.geometry.loc.y - 32.).max(0.),
            )
                .into(),
            preview.geometry.size.w,
        ) {
            push(label.into());
        }
        if let Some(buffer) = self.collection_overview.texture(ctx.r(), output, &preview) {
            let elem = PrimaryGpuTextureRenderElement(TextureRenderElement::from_texture_buffer(
                buffer,
                (0., 0.),
                1.,
                None,
                Some(preview.workspace.view_size()),
                Kind::Unspecified,
            ));
            let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
            let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
            if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                push(CollectionOverviewRenderElement::CachedPreview(elem));
            }
        } else {
            let mut render = |elem| {
                let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
                let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
                if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                    push(CollectionOverviewRenderElement::Workspace(elem));
                }
            };
            let xray_pos = XrayPos::new(preview.geometry.loc, preview.zoom);
            preview
                .workspace
                .render_floating(ctx.r(), xray_pos, false, &mut render);
            preview
                .workspace
                .render_scrolling(ctx.r(), xray_pos, false, &mut render);
            let background = preview.workspace.render_background();
            let background =
                RescaleRenderElement::from_element(background, (0, 0).into(), preview.zoom);
            let background =
                RelocateRenderElement::from_element(background, location, Relocate::Relative);
            if let Some(background) = CropRenderElement::from_element(background, scale, crop) {
                push(CollectionOverviewRenderElement::Background(background));
            }
        }
        let alpha = self
            .overview_progress
            .as_ref()
            .map_or(1., |p| p.value().clamp(0., 1.)) as f32;
        preview.workspace.render_shadow(ctx.renderer, &mut |elem| {
            let elem = elem.with_alpha(alpha);
            let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
            let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
            if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                push(CollectionOverviewRenderElement::Shadow(elem));
            }
        });
    }

    #[cfg(test)]
    pub(crate) fn collection_overview_cache_counts(&self) -> (u64, u64) {
        (
            self.collection_overview.preparations.get(),
            self.collection_overview.texture_builds.get(),
        )
    }

    pub fn collection_overview_is_enabled(&self) -> bool {
        self.options.layout.collections.enabled
    }

    pub fn show_workspace_collection(&mut self, reference: &niri_config::WorkspaceReference) {
        let workspace = match reference {
            niri_config::WorkspaceReference::Id(id) => {
                self.find_workspace_by_id(WorkspaceId::specific(*id))
            }
            niri_config::WorkspaceReference::Name(name) => self.find_workspace_by_name(name),
            niri_config::WorkspaceReference::Index(_) => None,
        };
        let Some((_, workspace)) = workspace else {
            return;
        };
        let workspace = workspace.id();
        let collection = self
            .collections
            .iter()
            .find(|c| c.workspaces.iter().any(|ws| ws.id() == workspace))
            .map(|c| c.id);
        if let Some(id) = collection {
            self.focus_collection(&niri_config::WorkspaceReference::Id(id));
        }
    }

    pub fn collection_overview_is_open(&self) -> bool {
        self.options.layout.collections.enabled && self.overview_open
    }

    fn collection_columns(&self, output: &Output) -> Vec<ColumnGeometry> {
        let Some(active) = self.collection_for_output(output) else {
            return Vec::new();
        };
        columns(
            self.collections
                .iter()
                .map(|c| c.id)
                .chain(
                    self.scratchpad
                        .has_windows()
                        .then_some(SCRATCHPAD_COLUMN_ID),
                )
                .filter(|id| {
                    *id == active || !self.monitors().any(|m| m.collection_id == Some(*id))
                }),
            active,
            output_size(output),
            self.overview_zoom(),
        )
    }

    pub(super) fn collection_previews(&self, output: &Output) -> Vec<Preview<'_, W>> {
        let size = output_size(output);
        let active = self.collection_for_output(output);
        let columns = self.collection_columns(output);
        let mut key = vec![
            size.w.to_bits(),
            size.h.to_bits(),
            self.overview_zoom().to_bits(),
        ];
        for column in &columns {
            if Some(column.id) == active || column.bounds.size.w <= 0. {
                continue;
            }
            key.extend([
                column.id,
                column.bounds.loc.x.to_bits(),
                column.bounds.size.w.to_bits(),
            ]);
            let workspaces: Vec<_> = if column.id == SCRATCHPAD_COLUMN_ID {
                vec![&self.scratchpad]
            } else {
                self.collection_workspaces(column.id).collect()
            };
            key.push(workspaces.len() as u64);
            key.push(if column.id == SCRATCHPAD_COLUMN_ID {
                self.scratchpad.id().get()
            } else {
                self.collection_active_workspace(column.id)
                    .map_or(0, WorkspaceId::get)
            });
            for workspace in workspaces {
                key.extend([
                    workspace.id().get(),
                    workspace.view_size().w.to_bits(),
                    workspace.view_size().h.to_bits(),
                ]);
            }
        }
        let cached = self
            .collection_overview
            .preview_geometry
            .borrow()
            .get(&key)
            .cloned();
        let geometries = if let Some(cached) = cached {
            cached
        } else {
            let mut geometries = Vec::new();
            for column in &columns {
                if Some(column.id) == active || column.bounds.size.w <= 0. {
                    continue;
                }
                let workspaces: Vec<_> = if column.id == SCRATCHPAD_COLUMN_ID {
                    vec![&self.scratchpad]
                } else {
                    self.collection_workspaces(column.id).collect()
                };
                let active_ws = if column.id == SCRATCHPAD_COLUMN_ID {
                    Some(self.scratchpad.id())
                } else {
                    self.collection_active_workspace(column.id)
                };
                let active_idx = workspaces
                    .iter()
                    .position(|ws| Some(ws.id()) == active_ws)
                    .unwrap_or(0);
                let heights: Vec<_> = workspaces
                    .iter()
                    .map(|ws| ws.view_size().h * column.bounds.size.w / ws.view_size().w.max(1.))
                    .collect();
                let mut positions = vec![0.; workspaces.len()];
                if let Some(height) = heights.get(active_idx) {
                    positions[active_idx] = (size.h - height) / 2.;
                    for idx in active_idx + 1..workspaces.len() {
                        positions[idx] = positions[idx - 1] + heights[idx - 1] + 24.;
                    }
                    for idx in (0..active_idx).rev() {
                        positions[idx] = positions[idx + 1] - heights[idx] - 24.;
                    }
                }
                for (idx, workspace) in workspaces.into_iter().enumerate() {
                    let zoom = column.bounds.size.w / workspace.view_size().w.max(1.);
                    geometries.push(PreviewGeometry {
                        collection: column.id,
                        workspace: workspace.id(),
                        is_scratchpad: column.id == SCRATCHPAD_COLUMN_ID,
                        geometry: Rectangle::new(
                            (column.bounds.loc.x, positions[idx]).into(),
                            Size::from((column.bounds.size.w, workspace.view_size().h * zoom)),
                        ),
                        zoom,
                    });
                }
            }
            let mut cache = self.collection_overview.preview_geometry.borrow_mut();
            if cache.len() > 64 {
                cache.clear();
            }
            cache.insert(key, geometries.clone());
            geometries
        };
        let mut previews = Vec::new();
        for geometry in geometries {
            let workspace = if geometry.is_scratchpad {
                Some(&self.scratchpad)
            } else {
                self.collection_workspaces(geometry.collection)
                    .find(|workspace| workspace.id() == geometry.workspace)
            };
            if let Some(workspace) = workspace {
                previews.push(Preview {
                    id: geometry.collection,
                    workspace,
                    geometry: geometry.geometry,
                    zoom: geometry.zoom,
                    is_scratchpad: geometry.is_scratchpad,
                });
            }
        }
        previews
    }

    pub(super) fn collection_workspace_under(
        &self,
        output: &Output,
        pos: Point<f64, Logical>,
        extended: bool,
    ) -> Option<&Workspace<W>> {
        if let Some(workspace) = self.monitor_for_output(output).and_then(|monitor| {
            monitor
                .workspace_under(pos)
                .filter(|(_, geometry)| geometry.contains(pos))
                .map(|(workspace, _)| workspace)
        }) {
            return Some(workspace);
        }
        if (16. ..56.).contains(&pos.y) {
            if let Some(column) = self
                .collection_columns(output)
                .into_iter()
                .find(|c| c.bounds.contains(pos))
            {
                let active = if column.id == SCRATCHPAD_COLUMN_ID {
                    Some(self.scratchpad.id())
                } else {
                    self.collection_active_workspace(column.id)
                };
                return self
                    .collection_workspaces(column.id)
                    .find(|ws| Some(ws.id()) == active)
                    .or_else(|| (column.id == SCRATCHPAD_COLUMN_ID).then_some(&self.scratchpad));
            }
        }
        let previews = self.collection_previews(output);
        previews
            .into_iter()
            .find(|p| {
                let mut bounds = p.geometry;
                if extended {
                    bounds.loc.x -= 12.;
                    bounds.size.w += 24.;
                }
                bounds.contains(pos)
            })
            .map(|p| p.workspace)
    }

    pub(super) fn collection_window_under(
        &self,
        output: &Output,
        pos: Point<f64, Logical>,
    ) -> Option<(&W, HitType)> {
        if let Some(preview) = self
            .collection_previews(output)
            .into_iter()
            .find(|p| p.geometry.contains(pos))
        {
            let (window, hit) = preview
                .workspace
                .window_under((pos - preview.geometry.loc).downscale(preview.zoom))?;
            return Some((window, hit.to_activate()));
        }
        self.monitor_for_output(output)
            .filter(|monitor| {
                monitor
                    .workspaces_with_render_geo()
                    .any(|(_, geometry)| geometry.contains(pos))
            })
            .and_then(|monitor| monitor.window_under(pos))
    }

    pub(super) fn collection_window_preview(
        &self,
        output: &Output,
        window: &W::Id,
    ) -> Option<(&Workspace<W>, Rectangle<f64, Logical>, f64)> {
        if let Some((workspace, geometry, zoom)) =
            self.monitor_for_output(output).and_then(|monitor| {
                monitor
                    .workspaces_with_render_geo()
                    .find(|(workspace, _)| workspace.has_window(window))
                    .map(|(workspace, geometry)| (workspace, geometry, monitor.overview_zoom()))
            })
        {
            return Some((workspace, geometry, zoom));
        }
        self.collection_previews(output)
            .into_iter()
            .find(|p| p.workspace.has_window(window))
            .map(|p| (p.workspace, p.geometry, p.zoom))
    }

    pub(super) fn drop_in_collection(
        &mut self,
        mut moving: InteractiveMoveData<W>,
    ) -> Option<InteractiveMoveData<W>> {
        let pos = moving.pointer_pos_within_output;
        let previews = self.collection_previews(&moving.output);
        let Some(index) = preview_drop_target(&previews, pos) else {
            return Some(moving);
        };
        let preview = &previews[index];
        let id = preview.id;
        let workspace_id = preview.workspace.id();
        let local = (pos - preview.geometry.loc).downscale(preview.zoom);
        let position = if moving.is_floating {
            InsertPosition::Floating
        } else {
            preview.workspace.scrolling_insert_position(local)
        };
        if moving.is_floating {
            moving.tile.floating_pos = Some(preview.workspace.floating_logical_to_size_frac(local));
            if let Some(size) = moving.tile.window().expected_size() {
                moving.tile.floating_window_size = Some(size);
            }
        }
        moving.tile.window().output_leave(&moving.output);
        let shown = self.monitors().any(|m| m.collection_id == Some(id));
        if shown {
            let mon = self
                .monitors_mut()
                .find(|m| m.collection_id == Some(id))
                .unwrap();
            let idx = mon
                .workspaces
                .iter()
                .position(|ws| ws.id() == workspace_id)
                .unwrap();
            match position {
                InsertPosition::InColumn(col, tile) => {
                    mon.add_tile_to_column(idx, col, Some(tile), moving.tile, true, false)
                }
                other => mon.add_tile(
                    moving.tile,
                    MonitorAddWindowTarget::Workspace {
                        id: workspace_id,
                        column_idx: match other {
                            InsertPosition::NewColumn(col) => Some(col),
                            _ => None,
                        },
                    },
                    ActivateWindow::No,
                    false,
                    moving.width,
                    moving.is_full_width,
                    moving.is_floating,
                ),
            }
        } else {
            let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
            let ws = collection
                .workspaces
                .iter_mut()
                .find(|ws| ws.id() == workspace_id)
                .unwrap();
            match position {
                InsertPosition::InColumn(col, tile) => {
                    ws.add_tile_to_column(col, Some(tile), moving.tile, false)
                }
                other => ws.add_tile(
                    moving.tile,
                    match other {
                        InsertPosition::NewColumn(col) => {
                            WorkspaceAddWindowTarget::NewColumnAt(col)
                        }
                        _ => WorkspaceAddWindowTarget::Auto,
                    },
                    ActivateWindow::No,
                    moving.width,
                    moving.is_full_width,
                    moving.is_floating,
                ),
            }
            ws.refresh(false, false);
        }
        self.clean_up_collections();
        None
    }

    /// Select a workspace by identity so a click in another collection cannot resolve against
    /// the current monitor's unrelated workspace index.
    pub fn select_overview_workspace(&mut self, output: &Output, workspace: WorkspaceId) {
        self.focus_output(output);
        if self.collection_overview_is_open() {
            let collection = self
                .collections
                .iter()
                .find(|c| {
                    self.collection_workspaces(c.id)
                        .any(|ws| ws.id() == workspace)
                })
                .map(|c| c.id);
            if let Some(id) = collection {
                self.focus_collection(&niri_config::WorkspaceReference::Id(id));
            }
        }
        if let Some(idx) = self
            .monitor_for_output(output)
            .and_then(|m| m.workspaces.iter().position(|ws| ws.id() == workspace))
        {
            self.toggle_overview_to_workspace(idx);
        }
    }

    pub fn render_collection_overview<R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output: &Output,
        _focus_ring: bool,
        push: &mut dyn FnMut(CollectionOverviewRenderElement<R>),
    ) {
        let scale = output.current_scale().fractional_scale();
        let crop = Rectangle::from_size(output_size(output)).to_physical_precise_round(scale);
        let active = self.collection_for_output(output);
        let columns = self.collection_columns(output);
        for collection in &self.collections {
            let Some(column) = columns.iter().find(|c| c.id == collection.id) else {
                continue;
            };
            let name = collection.name.clone().unwrap_or_else(|| {
                collection.original_output.clone().unwrap_or_else(|| {
                    format!(
                        "Collection {}",
                        self.collections
                            .iter()
                            .position(|c| c.id == collection.id)
                            .unwrap()
                            + 1
                    )
                })
            });
            let text = if active == Some(collection.id) {
                format!("● {name}")
            } else {
                name
            };
            if let Some(label) = self.collection_overview.label(
                ctx.renderer,
                collection.id,
                &text,
                scale,
                (column.bounds.loc.x, 24.).into(),
                column.bounds.size.w,
            ) {
                push(label.into());
            }
        }
        if let Some(column) = columns
            .iter()
            .find(|column| column.id == SCRATCHPAD_COLUMN_ID)
        {
            if let Some(label) = self.collection_overview.label(
                ctx.renderer,
                SCRATCHPAD_COLUMN_ID,
                "Scratchpad",
                scale,
                (column.bounds.loc.x, 24.).into(),
                column.bounds.size.w,
            ) {
                push(label.into());
            }
        }
        for preview in self.collection_previews(output) {
            // The collection currently shown on this output uses the monitor's normal render path
            // so its windows, workspace layers, shadows, hover behavior, and animation share one
            // transform. This function only renders hidden/detached side columns.
            if active == Some(preview.id) {
                continue;
            }
            // Invisible rows need neither render-element traversal nor GPU allocation.
            if preview
                .geometry
                .intersection(Rectangle::from_size(output_size(output)))
                .is_none()
            {
                continue;
            }
            let location = preview.geometry.loc.to_physical_precise_round(scale);
            let xray_pos = XrayPos::new(preview.geometry.loc, preview.zoom);
            if let Some(buffer) = self.collection_overview.texture(ctx.r(), output, &preview) {
                let elem =
                    PrimaryGpuTextureRenderElement(TextureRenderElement::from_texture_buffer(
                        buffer,
                        (0., 0.),
                        1.,
                        None,
                        Some(preview.workspace.view_size()),
                        Kind::Unspecified,
                    ));
                let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
                let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
                if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                    push(CollectionOverviewRenderElement::CachedPreview(elem));
                }
            } else {
                // Allocation/render failure preserves a correct native rendering path.
                let mut render = |elem| {
                    let elem =
                        RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
                    let elem =
                        RelocateRenderElement::from_element(elem, location, Relocate::Relative);
                    if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                        push(CollectionOverviewRenderElement::Workspace(elem));
                    }
                };
                preview
                    .workspace
                    .render_floating(ctx.r(), xray_pos, false, &mut render);
                preview
                    .workspace
                    .render_scrolling(ctx.r(), xray_pos, false, &mut render);
                let background = preview.workspace.render_background();
                let background =
                    RescaleRenderElement::from_element(background, (0, 0).into(), preview.zoom);
                let background =
                    RelocateRenderElement::from_element(background, location, Relocate::Relative);
                if let Some(background) = CropRenderElement::from_element(background, scale, crop) {
                    push(CollectionOverviewRenderElement::Background(background));
                }
            }
            let alpha =
                self.overview_progress
                    .as_ref()
                    .map_or(1., |progress| progress.value().clamp(0., 1.)) as f32;
            preview.workspace.render_shadow(ctx.renderer, &mut |elem| {
                let elem = elem.with_alpha(alpha);
                let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
                let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
                if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) {
                    push(CollectionOverviewRenderElement::Shadow(elem));
                }
            });
        }
        self.render_collection_drop_hint(ctx.renderer, output, push);
    }

    fn render_collection_drop_hint<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        output: &Output,
        push: &mut dyn FnMut(CollectionOverviewRenderElement<R>),
    ) {
        let Some(InteractiveMoveState::Moving(moving)) = &self.interactive_move else {
            return;
        };
        if moving.output != *output || self.options.layout.insert_hint.off {
            return;
        }
        let pos = moving.pointer_pos_within_output;
        let previews = self.collection_previews(output);
        let Some(index) = preview_drop_target(&previews, pos) else { return; };
        let preview = &previews[index];
        let local = (pos - preview.geometry.loc).downscale(preview.zoom);
        let position = if moving.is_floating { InsertPosition::Floating }
            else { preview.workspace.scrolling_insert_position(local) };
        let Some(area) = preview.workspace.insert_hint_area(position) else { return; };
        let scale = output.current_scale().fractional_scale();
        let transformed_area = Rectangle::new(
            preview.geometry.loc + area.loc.upscale(preview.zoom),
            area.size.upscale(preview.zoom),
        ).to_physical_precise_round(scale).to_logical(scale);
        let transformed_view = Rectangle::new(
            (preview.geometry.loc - transformed_area.loc),
            preview.workspace.view_size().upscale(preview.zoom),
        );
        let border_width = moving.tile.effective_border_width().unwrap_or(0.);
        let source_radius = moving.tile.window().geometry_corner_radius().expanded_by(border_width as f32);
        let radius = niri_config::CornerRadius {
            top_left: source_radius.top_left * preview.zoom as f32,
            top_right: source_radius.top_right * preview.zoom as f32,
            bottom_right: source_radius.bottom_right * preview.zoom as f32,
            bottom_left: source_radius.bottom_left * preview.zoom as f32,
        };
        let mut hint_cache = self.collection_overview.drop_hints.borrow_mut();
        let hint = hint_cache.entry(output.clone())
            .or_insert_with(|| InsertHintElement::new(self.options.layout.insert_hint));
        hint.update_config(self.options.layout.insert_hint);
        hint.update_render_elements(
            transformed_area.size,
            transformed_view,
            radius,
            scale,
        );
        let crop = preview.geometry
            .intersection(Rectangle::from_size(output_size(output)))
            .map(|geometry| geometry.to_physical_precise_round(scale));
        hint.render(renderer, transformed_area.loc, &mut |elem| {
            if let Some(elem) = crop.and_then(|crop| CropRenderElement::from_element(elem, scale, crop)) {
                push(CollectionOverviewRenderElement::InsertHint(elem));
            }
        });
    }
}

fn preview_drop_target<W: LayoutElement>(
    previews: &[Preview<'_, W>],
    pos: Point<f64, Logical>,
) -> Option<usize> {
    preview_drop_target_geometry(
        previews.iter().enumerate().map(|(index, preview)| (index, preview.is_scratchpad, preview.geometry)),
        pos,
    )
}

fn preview_drop_target_geometry(
    previews: impl Iterator<Item = (usize, bool, Rectangle<f64, Logical>)>,
    pos: Point<f64, Logical>,
) -> Option<usize> {
    previews
        .filter_map(|(index, is_scratchpad, geometry)| {
            (!is_scratchpad && pos.x >= geometry.loc.x && pos.x < geometry.loc.x + geometry.size.w)
                .then_some((index, geometry))
        })
        .min_by(|(_, a), (_, b)| {
            let distance = |geometry: &Rectangle<f64, Logical>| {
                (pos.y - geometry.loc.y - geometry.size.h / 2.).abs()
            };
            distance(a).total_cmp(&distance(b))
        })
        .map(|(index, _)| index)
}

impl<W: LayoutElement> Layout<W> {
    pub(super) fn has_collection_drop_target(&self, output: &Output, pos: Point<f64, Logical>) -> bool {
        let previews = self.collection_previews(output);
        preview_drop_target(&previews, pos).is_some()
    }
}

impl Overview {
    fn texture<W: LayoutElement, R: NiriRenderer>(
        &self,
        mut ctx: RenderCtx<R>,
        output: &Output,
        preview: &Preview<'_, W>,
    ) -> Option<TextureBuffer<GlesTexture>> {
        let mut ctx = ctx.as_gles();
        // fill_xray_elements marks content pending; its commit changes only on preparation.
        // Flush pending damage before the cache lookup or a changed wallpaper could stay stale.
        if let Some(xray) = ctx.xray {
            for buffer in [
                &xray.background[ctx.target as usize],
                &xray.backdrop[ctx.target as usize],
            ] {
                if !buffer.borrow_mut().prepare(ctx.renderer, false) {
                    return None;
                }
            }
        }
        let xray_stamp = |ctx: &RenderCtx<_>| {
            ctx.xray.map(|xray| {
                let background = xray.background[ctx.target as usize].borrow();
                let backdrop = xray.backdrop[ctx.target as usize].borrow();
                [
                    (background.id().clone(), background.commit()),
                    (backdrop.id().clone(), backdrop.commit()),
                ]
            })
        };
        let mut key = TextureKey {
            output: output.clone(),
            context: ctx.renderer.context_id(),
            target: ctx.target,
            workspace: preview.workspace.id(),
            revision: preview.workspace.overview_revision(),
            scale: output.current_scale().fractional_scale(),
            location: preview.geometry.loc,
            zoom: preview.zoom,
            xray: xray_stamp(&ctx),
            xray_geometry: ctx.xray.map(|xray| XrayGeometry {
                backdrop_color: xray.backdrop_color,
                workspaces: xray.workspaces.clone(),
            }),
        };
        if let Some(cached) = self.textures.borrow().iter().find(|entry| entry.key == key) {
            return Some(cached.buffer.clone());
        }
        let mut elements: Vec<
            HiddenSnapshotRenderElement<smithay::backend::renderer::gles::GlesRenderer>,
        > = Vec::new();
        let xray_pos = XrayPos::new(preview.geometry.loc, preview.zoom);
        let scale = key.scale * preview.zoom;
        ctx.scale_override = Some(scale.into());
        preview
            .workspace
            .render_floating(ctx.r(), xray_pos, false, &mut |elem| {
                elements.push(elem.into())
            });
        preview
            .workspace
            .render_scrolling(ctx.r(), xray_pos, false, &mut |elem| {
                elements.push(elem.into())
            });
        elements.push(preview.workspace.render_background().into());
        // Render at the final preview resolution, keeping source logical coordinates intact.
        let size = preview
            .workspace
            .view_size()
            .to_physical_precise_ceil(scale);
        let (texture, _sync) = render_to_texture(
            ctx.renderer,
            size,
            scale.into(),
            Transform::Normal,
            Fourcc::Abgr8888,
            elements.iter().rev(),
        )
        .ok()?;
        let buffer = TextureBuffer::from_texture(
            ctx.renderer,
            texture,
            scale,
            Transform::Normal,
            Vec::new(),
        );
        // Xray preparation can update buffer commits while collecting the elements.
        key.xray = xray_stamp(&ctx);
        let mut textures = self.textures.borrow_mut();
        textures.retain(|entry| {
            !(entry.key.output == key.output
                && entry.key.target == key.target
                && entry.key.workspace == key.workspace)
        });
        // One entry per live output/target/workspace. Clearing by entry count would thrash
        // every steady frame when many small hidden previews are visible.
        textures.push(PreviewTexture {
            key,
            buffer: buffer.clone(),
        });
        #[cfg(test)]
        self.texture_builds.set(self.texture_builds.get() + 1);
        Some(buffer)
    }

    pub(super) fn needs_prepare(&self, workspace: WorkspaceId, revision: u64) -> bool {
        let previous = self
            .prepared_revisions
            .borrow_mut()
            .insert(workspace, revision);
        let needs = previous != Some(revision);
        #[cfg(test)]
        if needs {
            self.preparations.set(self.preparations.get() + 1);
        }
        needs
    }

    pub(super) fn clear(&self) {
        self.labels.borrow_mut().clear();
        self.preview_geometry.borrow_mut().clear();
        self.prepared_revisions.borrow_mut().clear();
        self.textures.borrow_mut().clear();
        for hint in self.drop_hints.borrow_mut().values_mut() {
            hint.update_shaders();
        }
    }

    pub(super) fn retain_sources(
        &self,
        workspaces: impl Iterator<Item = WorkspaceId>,
        outputs: impl Iterator<Item = Output>,
    ) {
        let workspaces: HashSet<_> = workspaces.collect();
        let outputs: HashSet<_> = outputs.collect();
        self.prepared_revisions
            .borrow_mut()
            .retain(|id, _| workspaces.contains(id));
        self.textures.borrow_mut().retain(|entry| {
            workspaces.contains(&entry.key.workspace) && outputs.contains(&entry.key.output)
        });
    }

    fn label<R: NiriRenderer>(
        &self,
        renderer: &mut R,
        collection_id: u64,
        text: &str,
        scale: f64,
        location: Point<f64, Logical>,
        width: f64,
    ) -> Option<PrimaryGpuTextureRenderElement> {
        let width = (width * scale).ceil().max(1.) as i32;
        let key = (collection_id, format!("{text}\0{width}"), scale.to_bits());
        let mut labels = self.labels.borrow_mut();
        // Config reloads and output scale changes should not grow the cache without bound.
        if labels.len() > 128 {
            labels.clear();
        }
        if !labels.contains_key(&key) {
            let surface =
                ImageSurface::create(cairo::Format::ARgb32, width, (32. * scale).ceil() as i32)
                    .ok()?;
            let cr = cairo::Context::new(&surface).ok()?;
            let layout = pangocairo::functions::create_layout(&cr);
            let mut font = pango::FontDescription::from_string("sans 13");
            font.set_absolute_size(13. * scale * pango::SCALE as f64);
            layout.set_font_description(Some(&font));
            layout.set_width(width * pango::SCALE);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            layout.set_text(text);
            cr.set_source_rgba(1., 1., 1., 0.95);
            pangocairo::functions::show_layout(&cr, &layout);
            drop(layout);
            drop(cr);
            let size = (surface.width(), surface.height());
            let data = surface.take_data().ok()?;
            let buffer = TextureBuffer::from_memory(
                renderer.as_gles_renderer(),
                &data,
                Fourcc::Argb8888,
                size,
                false,
                scale,
                Transform::Normal,
                Vec::new(),
            )
            .ok()?;
            labels.insert(key.clone(), buffer);
        }
        Some(PrimaryGpuTextureRenderElement(
            TextureRenderElement::from_texture_buffer(
                labels[&key].clone(),
                location,
                1.,
                None,
                None,
                Kind::Unspecified,
            ),
        ))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnGeometry {
    pub id: u64,
    pub bounds: Rectangle<f64, Logical>,
}

/// Keep the current collection at its native monitor zoom and place hidden collections in the
/// remaining side space, alternating right and left.
pub(super) fn columns(
    ids: impl IntoIterator<Item = u64>,
    active: u64,
    size: Size<f64, Logical>,
    zoom: f64,
) -> Vec<ColumnGeometry> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    if ids.is_empty() {
        return Vec::new();
    }
    if let Some(idx) = ids.iter().position(|id| *id == active) {
        ids.rotate_left(idx);
    }
    let active_width = (size.w * zoom).clamp(0., size.w);
    if active_width >= size.w {
        return vec![ColumnGeometry {
            id: ids[0],
            bounds: Rectangle::new(Point::from((0., 0.)), size),
        }];
    }
    let active_left = (size.w - active_width) / 2.;
    let right_count = ids.len() / 2;
    let left_count = (ids.len() - 1) / 2;
    let available_side_width = (size.w - active_width) / 2.;
    let gap = if ids.len() > 1 {
        24_f64.min(available_side_width / ids.len() as f64)
    } else {
        0.
    };
    let side_width = |count: usize| {
        if count == 0 {
            0.
        } else {
            (((size.w - active_width) / 2.) - gap * count as f64).max(0.) / count as f64
        }
    };
    let right_width = side_width(right_count);
    let left_width = side_width(left_count);
    ids.into_iter()
        .enumerate()
        .map(|(idx, id)| {
            let (x, width) = if idx == 0 {
                (active_left, active_width)
            } else if idx % 2 == 1 {
                let slot = idx.div_ceil(2) - 1;
                (
                    active_left + active_width + gap + slot as f64 * (right_width + gap),
                    right_width,
                )
            } else {
                let slot = idx / 2 - 1;
                (
                    active_left - gap - (slot + 1) as f64 * left_width - slot as f64 * gap,
                    left_width,
                )
            };
            // Keep round-off at the outermost side columns inside the output bounds.
            let x = x.clamp(0., size.w);
            let width = width.clamp(0., size.w - x);
            ColumnGeometry {
                id,
                bounds: Rectangle::new(Point::from((x, 0.)), Size::from((width, size.h))),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_and_hit_distinct_collection() {
        for count in 1..20 {
            let columns = columns(1..=count, 1, (1280., 720.).into(), 0.5);
            for column in &columns {
                assert!(column.bounds.loc.x >= 0.);
                assert!(column.bounds.loc.x + column.bounds.size.w <= 1280.);
                let point = column.bounds.loc + column.bounds.size.to_point().downscale(2.);
                let hits: Vec<_> = columns
                    .iter()
                    .filter(|c| c.bounds.contains(point))
                    .collect();
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].id, column.id);
            }
            assert_eq!(
                columns[0].bounds.loc.x + columns[0].bounds.size.w / 2.,
                640.
            );
        }
    }

}
