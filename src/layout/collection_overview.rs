//! Shared collection-column geometry for rendering, pointer selection, and drag targets.

use std::cell::RefCell;
use std::collections::HashMap;

use pangocairo::cairo::{self, ImageSurface};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::utils::{CropRenderElement, Relocate, RelocateRenderElement, RescaleRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesTexture;
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Size, Transform};

use super::workspace::{Workspace, WorkspaceId, WorkspaceRenderElement};
use super::{HitType, Layout, LayoutElement};
use super::{ActivateWindow, InteractiveMoveData};
use super::monitor::{InsertPosition, MonitorAddWindowTarget};
use super::workspace::WorkspaceAddWindowTarget;
use crate::niri_render_elements;
use crate::render_helpers::primary_gpu_texture::PrimaryGpuTextureRenderElement;
use crate::render_helpers::renderer::NiriRenderer;
use crate::render_helpers::solid_color::SolidColorRenderElement;
use crate::render_helpers::texture::{TextureBuffer, TextureRenderElement};
use crate::render_helpers::xray::XrayPos;
use crate::render_helpers::RenderCtx;
use crate::utils::output_size;

#[derive(Debug, Default)]
pub(super) struct Overview {
    labels: RefCell<HashMap<(String, u64), TextureBuffer<GlesTexture>>>,
}

niri_render_elements! {
    CollectionOverviewRenderElement<R> => {
        Workspace = CropRenderElement<RelocateRenderElement<RescaleRenderElement<WorkspaceRenderElement<R>>>>,
        Background = CropRenderElement<RelocateRenderElement<RescaleRenderElement<SolidColorRenderElement>>>,
        Label = PrimaryGpuTextureRenderElement,
    }
}

struct Preview<'a, W: LayoutElement> {
    id: u64,
    workspace: &'a Workspace<W>,
    geometry: Rectangle<f64, Logical>,
    zoom: f64,
}

impl<W: LayoutElement> Layout<W> {
    pub fn collection_overview_is_open(&self) -> bool {
        self.options.layout.collections.enabled && self.overview_open
    }

    fn collection_columns(&self, output: &Output) -> Vec<ColumnGeometry> {
        let Some(active) = self.collection_for_output(output) else { return Vec::new(); };
        columns(self.collections.iter().map(|c| c.id), active, output_size(output), self.overview_zoom())
    }

    fn collection_previews(&self, output: &Output) -> Vec<Preview<'_, W>> {
        let size = output_size(output);
        let mut previews = Vec::new();
        for column in self.collection_columns(output) {
            let workspaces: Vec<_> = self.collection_workspaces(column.id).collect();
            let active = self.collection_active_workspace(column.id);
            let active_idx = workspaces.iter().position(|ws| Some(ws.id()) == active).unwrap_or(0);
            // Every stack uses its saved workspace sizes. A detached monitor may have had a
            // different aspect ratio; fitting its width preserves the actual window geometry.
            let height = workspaces.get(active_idx).map_or(size.h * column.zoom, |ws| {
                ws.view_size().h * column.bounds.size.w / ws.view_size().w.max(1.)
            });
            let step = height + 24.;
            for (idx, ws) in workspaces.into_iter().enumerate() {
                let zoom = column.bounds.size.w / ws.view_size().w.max(1.);
                let geometry = Rectangle::new(
                    (column.bounds.loc.x, (size.h - height) / 2. + (idx as f64 - active_idx as f64) * step).into(),
                    Size::from((column.bounds.size.w, ws.view_size().h * zoom)),
                );
                previews.push(Preview { id: column.id, workspace: ws, geometry, zoom });
            }
        }
        previews
    }

    pub(super) fn collection_workspace_under(&self, output: &Output, pos: Point<f64, Logical>, extended: bool) -> Option<&Workspace<W>> {
        if (16. ..56.).contains(&pos.y) {
            if let Some(column) = self.collection_columns(output).into_iter().find(|c| c.bounds.contains(pos)) {
                let active = self.collection_active_workspace(column.id);
                return self.collection_workspaces(column.id).find(|ws| Some(ws.id()) == active);
            }
        }
        let previews = self.collection_previews(output);
        previews.into_iter().find(|p| {
            let mut bounds = p.geometry;
            if extended {
                bounds.loc.x -= 12.;
                bounds.size.w += 24.;
            }
            bounds.contains(pos)
        }).map(|p| p.workspace)
    }

    pub(super) fn collection_window_under(&self, output: &Output, pos: Point<f64, Logical>) -> Option<(&W, HitType)> {
        let preview = self.collection_previews(output).into_iter().find(|p| p.geometry.contains(pos))?;
        let (window, hit) = preview.workspace.window_under((pos - preview.geometry.loc).downscale(preview.zoom))?;
        Some((window, hit.to_activate()))
    }

    pub(super) fn collection_window_preview(&self, output: &Output, window: &W::Id) -> Option<(&Workspace<W>, Rectangle<f64, Logical>, f64)> {
        self.collection_previews(output).into_iter().find(|p| p.workspace.has_window(window)).map(|p| (p.workspace, p.geometry, p.zoom))
    }

    pub(super) fn drop_in_collection(&mut self, mut moving: InteractiveMoveData<W>) -> Result<(), InteractiveMoveData<W>> {
        let pos = moving.pointer_pos_within_output;
        let Some(preview) = self.collection_previews(&moving.output).into_iter().find(|p| p.geometry.contains(pos)) else { return Err(moving); };
        let id = preview.id;
        let workspace_id = preview.workspace.id();
        let local = (pos - preview.geometry.loc).downscale(preview.zoom);
        let position = if moving.is_floating { InsertPosition::Floating } else { preview.workspace.scrolling_insert_position(local) };
        if moving.is_floating {
            moving.tile.floating_pos = Some(preview.workspace.floating_logical_to_size_frac(local));
            if let Some(size) = moving.tile.window().expected_size() { moving.tile.floating_window_size = Some(size); }
        }
        moving.tile.window().output_leave(&moving.output);
        let shown = self.monitors().any(|m| m.collection_id == Some(id));
        if shown {
            let mon = self.monitors_mut().find(|m| m.collection_id == Some(id)).unwrap();
            let idx = mon.workspaces.iter().position(|ws| ws.id() == workspace_id).unwrap();
            match position {
                InsertPosition::InColumn(col, tile) => mon.add_tile_to_column(idx, col, Some(tile), moving.tile, true, false),
                other => mon.add_tile(moving.tile, MonitorAddWindowTarget::Workspace { id: workspace_id, column_idx: match other { InsertPosition::NewColumn(col) => Some(col), _ => None } }, ActivateWindow::No, false, moving.width, moving.is_full_width, moving.is_floating),
            }
        } else {
            let collection = self.collections.iter_mut().find(|c| c.id == id).unwrap();
            let ws = collection.workspaces.iter_mut().find(|ws| ws.id() == workspace_id).unwrap();
            match position {
                InsertPosition::InColumn(col, tile) => ws.add_tile_to_column(col, Some(tile), moving.tile, false),
                other => ws.add_tile(moving.tile, match other { InsertPosition::NewColumn(col) => WorkspaceAddWindowTarget::NewColumnAt(col), _ => WorkspaceAddWindowTarget::Auto }, ActivateWindow::No, moving.width, moving.is_full_width, moving.is_floating),
            }
            ws.refresh(false, false);
        }
        self.clean_up_collections();
        Ok(())
    }

    /// Select a workspace by identity so a click in another collection cannot resolve against
    /// the current monitor's unrelated workspace index.
    pub fn select_overview_workspace(&mut self, output: &Output, workspace: WorkspaceId) {
        self.focus_output(output);
        if self.collection_overview_is_open() {
            let collection = self.collections.iter().find(|c| self.collection_workspaces(c.id).any(|ws| ws.id() == workspace)).map(|c| c.id);
            if let Some(id) = collection { self.focus_collection(&niri_config::WorkspaceReference::Id(id)); }
        }
        if let Some(idx) = self.monitor_for_output(output).and_then(|m| m.workspaces.iter().position(|ws| ws.id() == workspace)) {
            self.toggle_overview_to_workspace(idx);
        }
    }

    pub fn render_collection_overview<R: NiriRenderer>(&self, mut ctx: RenderCtx<R>, output: &Output, focus_ring: bool, push: &mut dyn FnMut(CollectionOverviewRenderElement<R>)) {
        let scale = output.current_scale().fractional_scale();
        let crop = Rectangle::from_size(output_size(output)).to_physical_precise_round(scale);
        let active = self.collection_for_output(output);
        for collection in &self.collections {
            let Some(column) = self.collection_columns(output).into_iter().find(|c| c.id == collection.id) else { continue; };
            let name = collection.name.clone().unwrap_or_else(|| {
                collection.original_output.clone().unwrap_or_else(|| format!("Collection {}", self.collections.iter().position(|c| c.id == collection.id).unwrap() + 1))
            });
            let text = if active == Some(collection.id) { format!("● {name}") } else { name };
            if let Some(label) = self.collection_overview.label(ctx.renderer, &text, scale, (column.bounds.loc.x, 24.).into()) {
                push(label.into());
            }
        }
        for preview in self.collection_previews(output) {
            let location = preview.geometry.loc.to_physical_precise_round(scale);
            let xray_pos = XrayPos::new(preview.geometry.loc, preview.zoom);
            let mut render = |elem| {
                let elem = RescaleRenderElement::from_element(elem, (0, 0).into(), preview.zoom);
                let elem = RelocateRenderElement::from_element(elem, location, Relocate::Relative);
                if let Some(elem) = CropRenderElement::from_element(elem, scale, crop) { push(CollectionOverviewRenderElement::Workspace(elem)); }
            };
            let ring = focus_ring && active == Some(preview.id);
            preview.workspace.render_floating(ctx.r(), xray_pos, ring, &mut render);
            preview.workspace.render_scrolling(ctx.r(), xray_pos, ring, &mut render);
            let background = preview.workspace.render_background();
            let background = RescaleRenderElement::from_element(background, (0, 0).into(), preview.zoom);
            let background = RelocateRenderElement::from_element(background, location, Relocate::Relative);
            if let Some(background) = CropRenderElement::from_element(background, scale, crop) { push(CollectionOverviewRenderElement::Background(background)); }
        }
    }
}

impl Overview {
    fn label<R: NiriRenderer>(&self, renderer: &mut R, text: &str, scale: f64, location: Point<f64, Logical>) -> Option<PrimaryGpuTextureRenderElement> {
        let key = (text.to_owned(), scale.to_bits());
        let mut labels = self.labels.borrow_mut();
        // Config reloads and output scale changes should not grow the cache without bound.
        if labels.len() > 128 { labels.clear(); }
        if !labels.contains_key(&key) {
            let surface = ImageSurface::create(cairo::Format::ARgb32, 512, (32. * scale).ceil() as i32).ok()?;
            let cr = cairo::Context::new(&surface).ok()?;
            let layout = pangocairo::functions::create_layout(&cr);
            let mut font = pango::FontDescription::from_string("sans 13");
            font.set_absolute_size(13. * scale * pango::SCALE as f64);
            layout.set_font_description(Some(&font));
            layout.set_width((500. * pango::SCALE as f64) as i32);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            layout.set_text(text);
            cr.set_source_rgba(1., 1., 1., 0.95);
            pangocairo::functions::show_layout(&cr, &layout);
            drop(layout);
            drop(cr);
            let size = (surface.width(), surface.height());
            let data = surface.take_data().ok()?;
            let buffer = TextureBuffer::from_memory(renderer.as_gles_renderer(), &data, Fourcc::Argb8888, size, false, scale, Transform::Normal, Vec::new()).ok()?;
            labels.insert(key.clone(), buffer);
        }
        Some(PrimaryGpuTextureRenderElement(TextureRenderElement::from_texture_buffer(labels[&key].clone(), location, 1., None, None, Kind::Unspecified)))
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ColumnGeometry {
    pub id: u64,
    pub bounds: Rectangle<f64, Logical>,
    pub zoom: f64,
}

/// Keep the current collection centered, placing other columns alternately right and left.
/// Shrink the previews only as far as necessary to fit every collection on this output.
pub(super) fn columns(
    ids: impl IntoIterator<Item = u64>,
    active: u64,
    size: Size<f64, Logical>,
    zoom: f64,
) -> Vec<ColumnGeometry> {
    let mut ids: Vec<_> = ids.into_iter().collect();
    if let Some(idx) = ids.iter().position(|id| *id == active) {
        ids.rotate_left(idx);
    }
    let slots = 2 * (ids.len() / 2) + 1;
    let gap = 24_f64.min(size.w / (slots as f64 * 8.));
    let width = (size.w / slots as f64 - gap).max(1.).min(size.w * zoom);
    let zoom = width / size.w.max(1.);
    ids.into_iter().enumerate().map(|(idx, id)| {
        let offset = if idx == 0 { 0. } else {
            let step = idx.div_ceil(2) as f64;
            if idx % 2 == 1 { step } else { -step }
        };
        ColumnGeometry {
            id,
            bounds: Rectangle::new(
                Point::from(((size.w - width) / 2. + offset * (width + gap), 0.)),
                Size::from((width, size.h)),
            ),
            zoom,
        }
    }).collect()
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
                let hits: Vec<_> = columns.iter().filter(|c| c.bounds.contains(point)).collect();
                assert_eq!(hits.len(), 1);
                assert_eq!(hits[0].id, column.id);
            }
            assert_eq!(columns[0].bounds.loc.x + columns[0].bounds.size.w / 2., 640.);
        }
    }
}
