// SPDX-License-Identifier: GPL-3.0-only
//! GPL-3.0-only reference contour implementation for Bead compositor reveals.
//! Ported from Nacre's GPL-3.0-only `crates/nacre-shell/src/contour.rs` at
//! source base 88cb0f2f (it derives from drawers/contour.js e0a81ce7).
use std::collections::BTreeMap;

use tiny_skia::{
    FillRule, GradientStop, LineCap, LinearGradient, Paint, Path, PathBuilder, Pixmap, PixmapPaint,
    SpreadMode, Stroke, Transform,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
}

fn raster_transform(output: Rect, crop: Rect, width: u32, height: u32) -> Transform {
    let scale_x = width as f32 / crop.w;
    let scale_y = height as f32 / crop.h;
    let crop_x = crop.x - output.x;
    let crop_y = crop.y - output.y;
    Transform::from_row(
        scale_x,
        0.,
        0.,
        scale_y,
        -crop_x * scale_x,
        -crop_y * scale_y,
    )
}

/// Immutable compositor-side reveal descriptor. The animation clock is shared
/// with the layer's atlas planes; `sample` accepts that clock's eased progress.
#[derive(Clone, Debug, PartialEq)]
pub struct RevealDescriptor {
    pub output_bounds: Rect,
    pub crop: Rect,
    pub static_rects: Vec<Rect>,
    pub holes: Vec<Rect>,
    pub animated_from: Rect,
    pub animated_to: Rect,
    pub content_clip: Option<(Rect, Rect)>,
    pub radii_from: [f32; 4],
    pub radii_to: [f32; 4],
    pub corner_mode: CornerMode,
    pub square_edges: u32,
    pub fill: [u8; 4],
    pub opacity_plane: u32,
    pub visible: bool,
    pub rim: Option<RimStyle>,
    pub shadow: Option<ShadowStyle>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CornerMode {
    Circular,
    QuadraticCut,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShadowStyle {
    pub color: [u8; 4],
    pub offset: (f32, f32),
    pub blur_radius: f32,
    pub spread: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RimStyle {
    pub colors: [[u8; 4]; 3],
    pub stops: [f32; 3],
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub width: f32,
    pub opacity: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevealSample {
    pub body: Rect,
    pub radii: [f32; 4],
    pub content_clip: Option<Rect>,
}

impl RevealDescriptor {
    pub fn empty() -> Self {
        Self {
            output_bounds: Rect::new(0., 0., 0., 0.),
            crop: Rect::new(0., 0., 0., 0.),
            static_rects: Vec::new(),
            holes: Vec::new(),
            animated_from: Rect::new(0., 0., 0., 0.),
            animated_to: Rect::new(0., 0., 0., 0.),
            content_clip: None,
            radii_from: [0.; 4],
            radii_to: [0.; 4],
            corner_mode: CornerMode::Circular,
            square_edges: 0,
            fill: [0; 4],
            opacity_plane: 0,
            visible: true,
            rim: None,
            shadow: None,
        }
    }

    pub fn is_valid(&self) -> bool {
        let rect_valid = |r: Rect, allow_zero: bool| {
            [r.x, r.y, r.w, r.h]
                .iter()
                .all(|v| v.is_finite() && v.abs() <= 32_768.)
                && (allow_zero || (r.w > 0. && r.h > 0.))
                && r.w >= 0.
                && r.h >= 0.
        };
        let values_valid = |values: &[f32]| {
            values
                .iter()
                .all(|v| v.is_finite() && *v >= 0. && *v <= 32_768.)
        };
        let coordinates_valid =
            |values: &[f32]| values.iter().all(|v| v.is_finite() && v.abs() <= 32_768.);
        rect_valid(self.output_bounds, true)
            && rect_valid(self.crop, true)
            && ((self.output_bounds.w == 0. && self.output_bounds.h == 0.)
                || (self.output_bounds.w > 0. && self.output_bounds.h > 0.))
            && ((self.crop.w == 0. && self.crop.h == 0.) || (self.crop.w > 0. && self.crop.h > 0.))
            && rect_valid(self.animated_from, true)
            && rect_valid(self.animated_to, true)
            && self
                .content_clip
                .is_none_or(|(from, to)| rect_valid(from, true) && rect_valid(to, true))
            && self.static_rects.len() <= 16
            && self.holes.len() <= 16
            && self.static_rects.iter().all(|r| rect_valid(*r, false))
            && self.holes.iter().all(|r| rect_valid(*r, false))
            && values_valid(&self.radii_from)
            && values_valid(&self.radii_to)
            && self.square_edges & !0x0f == 0
            && self.rim.as_ref().is_none_or(|rim| {
                values_valid(&[
                    rim.stops[0],
                    rim.stops[1],
                    rim.stops[2],
                    rim.width,
                    rim.opacity,
                ]) && coordinates_valid(&[rim.start.0, rim.start.1, rim.end.0, rim.end.1])
                    && rim.stops[0] <= rim.stops[1]
                    && rim.stops[1] <= rim.stops[2]
                    && rim.stops[2] <= 1.
                    && rim.opacity <= 1.
                    && (rim.width == 0. || rim.opacity == 0. || rim.start != rim.end)
            })
            && self.shadow.as_ref().is_none_or(|shadow| {
                values_valid(&[shadow.blur_radius])
                    && coordinates_valid(&[shadow.offset.0, shadow.offset.1, shadow.spread])
            })
            && (self.shadow.is_none()
                || (self.static_rects.is_empty()
                    && self.holes.is_empty()
                    && self.corner_mode == CornerMode::Circular))
    }

    pub fn sample(&self, eased_progress: f64) -> RevealSample {
        let t = eased_progress.clamp(0., 1.) as f32;
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        RevealSample {
            body: Rect::new(
                lerp(self.animated_from.x, self.animated_to.x),
                lerp(self.animated_from.y, self.animated_to.y),
                lerp(self.animated_from.w, self.animated_to.w),
                lerp(self.animated_from.h, self.animated_to.h),
            ),
            radii: std::array::from_fn(|i| {
                (lerp(self.radii_from[i], self.radii_to[i]) * 1000.).round() / 1000.
            }),
            content_clip: self.content_clip.map(|(from, to)| {
                Rect::new(
                    lerp(from.x, to.x),
                    lerp(from.y, to.y),
                    lerp(from.w, to.w),
                    lerp(from.h, to.h),
                )
            }),
        }
    }

    pub fn contains(&self, point: (f64, f64), eased_progress: f64) -> bool {
        let sample = self.sample(eased_progress);
        let mut filled = true;
        if self.visible {
            let to_local = |rect: Rect| {
                Rect::new(
                    rect.x - self.output_bounds.x,
                    rect.y - self.output_bounds.y,
                    rect.w,
                    rect.h,
                )
            };
            let mut rects = self.static_rects.clone();
            rects.push(sample.body);
            let Ok(contour) = geometry_with_style(
                self.output_bounds.w as f64,
                self.output_bounds.h as f64,
                &rects.iter().copied().map(to_local).collect::<Vec<_>>(),
                sample.radii.map(f64::from),
                self.corner_mode,
                self.square_edges,
            ) else {
                return false;
            };
            let holes = self.holes.iter().copied().map(to_local).collect::<Vec<_>>();
            let Ok(hole_geometry) = geometry(
                self.output_bounds.w as f64,
                self.output_bounds.h as f64,
                &holes,
                0.,
            ) else {
                return false;
            };
            let Some(path) =
                path_from_svg_commands(&format!("{} {}", contour.body, hole_geometry.body))
            else {
                return false;
            };
            filled = path_contains_even_odd(
                &path,
                (
                    point.0 - self.output_bounds.x as f64,
                    point.1 - self.output_bounds.y as f64,
                ),
            );
        }
        if !filled {
            return false;
        }
        if self.visible
            && sample.content_clip.is_some_and(|clip| {
                point.0 < clip.x as f64
                    || point.1 < clip.y as f64
                    || point.0 >= (clip.x + clip.w) as f64
                    || point.1 >= (clip.y + clip.h) as f64
            })
        {
            return false;
        }
        true
    }

    /// Rasterize only the bounded reveal crop using Bead/Nacre's tiny-skia
    /// reference renderer. The returned RGBA pixels are premultiplied.
    pub fn rasterize(
        &self,
        eased_progress: f64,
        scale: f32,
        surface_size: (f32, f32),
    ) -> Option<(Pixmap, Rect)> {
        self.rasterize_in_crop(eased_progress, scale, surface_size, self.crop)
    }

    /// Rasterize a bounded subrectangle while keeping paths and gradients in the original
    /// output coordinate space. Used to bake a reusable growing-edge template.
    pub fn rasterize_in_crop(
        &self,
        eased_progress: f64,
        scale: f32,
        surface_size: (f32, f32),
        requested_crop: Rect,
    ) -> Option<(Pixmap, Rect)> {
        if !self.is_valid() || !scale.is_finite() || scale <= 0. {
            return None;
        }
        let sample = self.sample(eased_progress);
        let output = self.output_bounds;
        // Shadow planes are rendered separately by Niri's Gaussian shadow shader.
        let left = self.crop.x.max(requested_crop.x).max(output.x).max(0.);
        let top = self.crop.y.max(requested_crop.y).max(output.y).max(0.);
        let right = (self.crop.x + self.crop.w)
            .min(requested_crop.x + requested_crop.w)
            .min(output.x + output.w)
            .min(surface_size.0);
        let bottom = (self.crop.y + self.crop.h)
            .min(requested_crop.y + requested_crop.h)
            .min(output.y + output.h)
            .min(surface_size.1);
        if right <= left || bottom <= top {
            return None;
        }
        let crop = Rect::new(left, top, right - left, bottom - top);
        let width = (crop.w * scale).round() as u32;
        let height = (crop.h * scale).round() as u32;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) * 4 > 64 * 1024 * 1024
        {
            return None;
        }
        let mut pixmap = Pixmap::new(width, height)?;
        let to_local = |rect: Rect| Rect::new(rect.x - output.x, rect.y - output.y, rect.w, rect.h);
        let mut body_rects = self.static_rects.clone();
        body_rects.push(sample.body);
        for rect in &mut body_rects {
            *rect = to_local(*rect);
        }
        let contour = geometry_with_style(
            output.w as f64,
            output.h as f64,
            &body_rects,
            sample.radii.map(f64::from),
            self.corner_mode,
            self.square_edges,
        )
        .ok()?;
        let holes = self.holes.iter().map(|r| to_local(*r)).collect::<Vec<_>>();
        let hole_path = geometry(output.w as f64, output.h as f64, &holes, 0.)
            .ok()?
            .body;
        let body_path = path_from_svg_commands(&format!("{} {hole_path}", contour.body))?;
        // Match svg_pixmap's root transform: physical raster dimensions determine
        // each axis scale, which can differ slightly from nominal scale after rounding.
        let transform = raster_transform(output, crop, width, height);
        let mut fill = Paint::default();
        fill.set_color(tiny_skia::Color::from_rgba8(
            self.fill[0],
            self.fill[1],
            self.fill[2],
            self.fill[3],
        ));
        fill.anti_alias = true;
        pixmap.fill_path(&body_path, &fill, FillRule::EvenOdd, transform, None);
        if let Some(rim) = &self.rim {
            if rim.width > 0. && rim.opacity > 0. {
                let path = path_from_svg_commands(&contour.rim)?;
                let stops = (0..3)
                    .map(|i| {
                        GradientStop::new(
                            rim.stops[i],
                            tiny_skia::Color::from_rgba8(
                                rim.colors[i][0],
                                rim.colors[i][1],
                                rim.colors[i][2],
                                rim.colors[i][3],
                            ),
                        )
                    })
                    .collect::<Vec<_>>();
                let start =
                    tiny_skia::Point::from_xy(rim.start.0 - output.x, rim.start.1 - output.y);
                let end = tiny_skia::Point::from_xy(rim.end.0 - output.x, rim.end.1 - output.y);
                let mut paint = Paint::default();
                paint.shader =
                    LinearGradient::new(start, end, stops, SpreadMode::Pad, Transform::identity())?;
                paint.anti_alias = true;
                let stroke = Stroke {
                    width: rim.width,
                    line_cap: LineCap::Butt,
                    ..Stroke::default()
                };
                draw_rim_group(&mut pixmap, &path, &paint, &stroke, transform, rim.opacity);
            }
        }
        Some((pixmap, crop))
    }
}

fn draw_rim_group(
    target: &mut Pixmap,
    path: &Path,
    paint: &Paint<'_>,
    stroke: &Stroke,
    transform: Transform,
    opacity: f32,
) {
    let group_paint = PixmapPaint {
        opacity,
        quality: tiny_skia::FilterQuality::Nearest,
        ..Default::default()
    };
    let isolated = (|| {
        let stroked = path.stroke(stroke, 1.)?;
        let bbox = stroked.compute_tight_bounds()?.transform(transform)?;
        let x = (bbox.x().floor() as i32).checked_sub(2)?;
        let y = (bbox.y().floor() as i32).checked_sub(2)?;
        let width = (bbox.width().ceil() as u32).checked_add(4)?;
        let height = (bbox.height().ceil() as u32).checked_add(4)?;
        let mut bounds = tiny_skia::IntRect::from_xywh(x, y, width, height)?;
        let canvas_width = target.width();
        let canvas_height = target.height();
        let max_bounds = tiny_skia::IntRect::from_xywh(
            -(canvas_width as i32) * 2,
            -(canvas_height as i32) * 2,
            canvas_width.checked_mul(5)?,
            canvas_height.checked_mul(5)?,
        )?;
        bounds = fit_int_rect(bounds, max_bounds)?;
        let byte_len = u64::from(bounds.width()) * u64::from(bounds.height()) * 4;
        if byte_len > 64 * 1024 * 1024 {
            return None;
        }
        let shift = Transform::from_translate(-(bounds.x() as f32), -(bounds.y() as f32));
        let layer_transform = shift.pre_concat(transform);
        let mut layer = Pixmap::new(bounds.width(), bounds.height())?;
        layer.stroke_path(path, paint, stroke, layer_transform, None);
        Some((layer, bounds.x(), bounds.y()))
    })();

    if let Some((layer, x, y)) = isolated {
        target.draw_pixmap(
            x,
            y,
            layer.as_ref(),
            &group_paint,
            Transform::identity(),
            None,
        );
    } else if let Some(mut layer) = Pixmap::new(target.width(), target.height()) {
        // Keep a bounded crop-sized group if the legacy expanded stroke layer is oversized.
        layer.stroke_path(path, paint, stroke, transform, None);
        target.draw_pixmap(
            0,
            0,
            layer.as_ref(),
            &group_paint,
            Transform::identity(),
            None,
        );
    }
}

fn fit_int_rect(
    rect: tiny_skia::IntRect,
    bounds: tiny_skia::IntRect,
) -> Option<tiny_skia::IntRect> {
    tiny_skia::IntRect::from_ltrb(
        rect.left().max(bounds.left()),
        rect.top().max(bounds.top()),
        rect.right().min(bounds.right()),
        rect.bottom().min(bounds.bottom()),
    )
}

fn path_contains_even_odd(path: &Path, point: (f64, f64)) -> bool {
    use tiny_skia::PathSegment;

    let mut crossings = 0u32;
    let mut current = (0., 0.);
    let mut start = current;
    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                current = (p.x as f64, p.y as f64);
                start = current;
            }
            PathSegment::LineTo(p) => {
                let end = (p.x as f64, p.y as f64);
                if line_crosses(current, end, point) {
                    crossings ^= 1;
                }
                current = end;
            }
            PathSegment::QuadTo(control, end) => {
                let control = (control.x as f64, control.y as f64);
                let end = (end.x as f64, end.y as f64);
                if curve_crosses(current, control, None, end, point) {
                    crossings ^= 1;
                }
                current = end;
            }
            PathSegment::CubicTo(c1, c2, end) => {
                let c1 = (c1.x as f64, c1.y as f64);
                let c2 = (c2.x as f64, c2.y as f64);
                let end = (end.x as f64, end.y as f64);
                if curve_crosses(current, c1, Some(c2), end, point) {
                    crossings ^= 1;
                }
                current = end;
            }
            PathSegment::Close => {
                if line_crosses(current, start, point) {
                    crossings ^= 1;
                }
                current = start;
            }
        }
    }
    crossings != 0
}

fn line_crosses(a: (f64, f64), b: (f64, f64), point: (f64, f64)) -> bool {
    if (a.1 <= point.1 && point.1 < b.1) || (b.1 <= point.1 && point.1 < a.1) {
        let x = a.0 + (point.1 - a.1) * (b.0 - a.0) / (b.1 - a.1);
        return x > point.0;
    }
    false
}

fn curve_crosses(
    a: (f64, f64),
    c1: (f64, f64),
    c2: Option<(f64, f64)>,
    b: (f64, f64),
    point: (f64, f64),
) -> bool {
    if !((a.1 <= point.1 && point.1 < b.1) || (b.1 <= point.1 && point.1 < a.1)) {
        return false;
    }
    let eval = |t: f64| {
        let u = 1. - t;
        if let Some(c2) = c2 {
            let x = u.powi(3) * a.0
                + 3. * u.powi(2) * t * c1.0
                + 3. * u * t.powi(2) * c2.0
                + t.powi(3) * b.0;
            let y = u.powi(3) * a.1
                + 3. * u.powi(2) * t * c1.1
                + 3. * u * t.powi(2) * c2.1
                + t.powi(3) * b.1;
            (x, y)
        } else {
            let x = u.powi(2) * a.0 + 2. * u * t * c1.0 + t.powi(2) * b.0;
            let y = u.powi(2) * a.1 + 2. * u * t * c1.1 + t.powi(2) * b.1;
            (x, y)
        }
    };
    let increasing = b.1 > a.1;
    let mut low = 0.;
    let mut high = 1.;
    for _ in 0..32 {
        let middle = (low + high) / 2.;
        let y = eval(middle).1;
        if (y < point.1) == increasing {
            low = middle;
        } else {
            high = middle;
        }
    }
    eval((low + high) / 2.).0 > point.0
}

fn path_from_svg_commands(input: &str) -> Option<Path> {
    let bytes = input.as_bytes();
    let mut pos = 0;
    let mut builder = PathBuilder::new();
    let mut command = b' ';
    let number = |bytes: &[u8], pos: &mut usize| -> Option<f32> {
        while *pos < bytes.len() && bytes[*pos].is_ascii_whitespace() {
            *pos += 1;
        }
        let start = *pos;
        while *pos < bytes.len()
            && !bytes[*pos].is_ascii_whitespace()
            && !bytes[*pos].is_ascii_alphabetic()
        {
            *pos += 1;
        }
        std::str::from_utf8(bytes.get(start..*pos)?)
            .ok()?
            .parse()
            .ok()
    };
    while pos < bytes.len() {
        while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
            pos += 1;
        }
        if pos == bytes.len() {
            break;
        }
        if bytes[pos].is_ascii_alphabetic() {
            command = bytes[pos];
            pos += 1;
        }
        match command {
            b'M' => {
                builder.move_to(number(bytes, &mut pos)?, number(bytes, &mut pos)?);
                command = b'L';
            }
            b'L' => builder.line_to(number(bytes, &mut pos)?, number(bytes, &mut pos)?),
            b'Q' => builder.quad_to(
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
            ),
            b'C' => builder.cubic_to(
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
                number(bytes, &mut pos)?,
            ),
            b'Z' => builder.close(),
            _ => return None,
        }
    }
    builder.finish()
}
#[derive(Clone, Copy, Debug, PartialEq)]
struct Point(f64, f64);
impl Point {
    fn key(self) -> (i64, i64) {
        (
            (self.0 * 1000.).round() as i64,
            (self.1 * 1000.).round() as i64,
        )
    }
}
struct Edge {
    a: Point,
    b: Point,
    direction: usize,
    used: bool,
}
#[derive(Debug, Default)]
pub struct Geometry {
    pub body: String,
    pub rim: String,
}
fn round(n: f64) -> f64 {
    (n * 1000.).round() / 1000.
}
fn area(points: &[Point]) -> f64 {
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let n = points[(i + 1) % points.len()];
            p.0 * n.1 - n.0 * p.1
        })
        .sum::<f64>()
        / 2.
}
fn outer(a: Point, b: Point, width: f64, height: f64, square_edges: u32) -> bool {
    a.0 == b.0 && ((a.0 == 0. && square_edges & 1 != 0) || (a.0 == width && square_edges & 4 != 0))
        || a.1 == b.1
            && ((a.1 == 0. && square_edges & 2 != 0) || (a.1 == height && square_edges & 8 != 0))
}
fn corner_radius(p: Point, a: Point, b: Point, radii: [f64; 4]) -> f64 {
    let dx = (a.0 - p.0).signum() + (b.0 - p.0).signum();
    let dy = (a.1 - p.1).signum() + (b.1 - p.1).signum();
    match (dx.total_cmp(&0.), dy.total_cmp(&0.)) {
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Greater) => radii[0], // TL
        (std::cmp::Ordering::Less, std::cmp::Ordering::Greater) => radii[1],    // TR
        (std::cmp::Ordering::Less, std::cmp::Ordering::Less) => radii[2],       // BR
        (std::cmp::Ordering::Greater, std::cmp::Ordering::Less) => radii[3],    // BL
        _ => radii.into_iter().fold(0., f64::max),
    }
}
fn path(
    points: &[Point],
    closed: bool,
    width: f64,
    height: f64,
    radii: [f64; 4],
    corner_mode: CornerMode,
    square_edges: u32,
) -> String {
    if points.len() < 2 {
        return String::new();
    }
    let pair = |p: Point| format!("{} {}", round(p.0), round(p.1));
    let corners = points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            if !closed && (i == 0 || i == points.len() - 1) {
                return (*p, *p);
            }
            let on_square_edge = (p.0 == 0. && square_edges & 1 != 0)
                || (p.1 == 0. && square_edges & 2 != 0)
                || (p.0 == width && square_edges & 4 != 0)
                || (p.1 == height && square_edges & 8 != 0);
            let r = if on_square_edge {
                0.
            } else {
                corner_radius(
                    *p,
                    points[(i + points.len() - 1) % points.len()],
                    points[(i + 1) % points.len()],
                    radii,
                )
            };
            let previous = points[(i + points.len() - 1) % points.len()];
            let next = points[(i + 1) % points.len()];
            let r = if corner_mode == CornerMode::Circular {
                r.min(((p.0 - previous.0).abs() + (p.1 - previous.1).abs()) / 2.)
                    .min(((next.0 - p.0).abs() + (next.1 - p.1).abs()) / 2.)
            } else {
                r
            };
            let toward = |q: Point| {
                let dx = q.0 - p.0;
                let dy = q.1 - p.1;
                let cut = if corner_mode == CornerMode::Circular {
                    r
                } else {
                    r.min((dx.abs() + dy.abs()) / 2.)
                };
                Point(
                    p.0 + if dx == 0. { 0. } else { dx.signum() * cut },
                    p.1 + if dy == 0. { 0. } else { dy.signum() * cut },
                )
            };
            (
                toward(points[(i + points.len() - 1) % points.len()]),
                toward(points[(i + 1) % points.len()]),
            )
        })
        .collect::<Vec<_>>();
    let mut result = format!("M{}", pair(corners[0].0));
    for (i, (entry, exit)) in corners.iter().enumerate() {
        if i > 0 {
            result.push_str(&format!(" L{}", pair(*entry)));
        }
        match corner_mode {
            CornerMode::QuadraticCut => {
                result.push_str(&format!(" Q{} {}", pair(points[i]), pair(*exit)));
            }
            CornerMode::Circular => {
                let previous = points[(i + points.len() - 1) % points.len()];
                let next = points[(i + 1) % points.len()];
                let p = points[i];
                let on_square_edge = (p.0 == 0. && square_edges & 1 != 0)
                    || (p.1 == 0. && square_edges & 2 != 0)
                    || (p.0 == width && square_edges & 4 != 0)
                    || (p.1 == height && square_edges & 8 != 0);
                let mut r = if on_square_edge || (!closed && (i == 0 || i == points.len() - 1)) {
                    0.
                } else {
                    corner_radius(p, previous, next, radii)
                };
                r = r
                    .min(((p.0 - previous.0).abs() + (p.1 - previous.1).abs()) / 2.)
                    .min(((next.0 - p.0).abs() + (next.1 - p.1).abs()) / 2.);
                let k = 0.552_284_75;
                let control_1 = Point(
                    entry.0 + (p.0 - previous.0).signum() * r * k,
                    entry.1 + (p.1 - previous.1).signum() * r * k,
                );
                let control_2 = Point(
                    exit.0 - (next.0 - p.0).signum() * r * k,
                    exit.1 - (next.1 - p.1).signum() * r * k,
                );
                result.push_str(&format!(
                    " C{} {} {}",
                    pair(control_1),
                    pair(control_2),
                    pair(*exit)
                ));
            }
        }
    }
    if closed {
        result.push_str(" Z");
    }
    result
}

/// FrameLips currently supplies no rim handles, exactly as in the reference.
/// This uses no handle approximation; handle geometry must be ported if enabled.
pub fn geometry(
    width: f64,
    height: f64,
    rectangles: &[Rect],
    radius: f64,
) -> Result<Geometry, String> {
    geometry_with_style(
        width,
        height,
        rectangles,
        [radius; 4],
        CornerMode::QuadraticCut,
        0x0f,
    )
}

pub fn geometry_with_radii(
    width: f64,
    height: f64,
    rectangles: &[Rect],
    radii: [f64; 4],
) -> Result<Geometry, String> {
    geometry_with_style(
        width,
        height,
        rectangles,
        radii,
        CornerMode::QuadraticCut,
        0x0f,
    )
}

pub fn geometry_with_style(
    width: f64,
    height: f64,
    rectangles: &[Rect],
    radii: [f64; 4],
    corner_mode: CornerMode,
    square_edges: u32,
) -> Result<Geometry, String> {
    if width <= 0. || height <= 0. {
        return Ok(Geometry::default());
    }
    let boxes = rectangles
        .iter()
        .map(|r| {
            (
                round((r.x as f64).clamp(0., width)),
                round((r.y as f64).clamp(0., height)),
                round(((r.x + r.w) as f64).clamp(0., width)),
                round(((r.y + r.h) as f64).clamp(0., height)),
            )
        })
        .filter(|(x, y, r, b)| r > x && b > y)
        .collect::<Vec<_>>();
    if boxes.is_empty() {
        return Ok(Geometry::default());
    }
    let mut xs = vec![0., width];
    let mut ys = vec![0., height];
    for &(x, y, r, b) in &boxes {
        xs.extend([x, r]);
        ys.extend([y, b]);
    }
    xs.sort_by(f64::total_cmp);
    xs.dedup();
    ys.sort_by(f64::total_cmp);
    ys.dedup();
    let occupied = ys
        .windows(2)
        .map(|y| {
            xs.windows(2)
                .map(|x| {
                    let px = (x[0] + x[1]) / 2.;
                    let py = (y[0] + y[1]) / 2.;
                    boxes
                        .iter()
                        .any(|&(x, y, r, b)| px > x && px < r && py > y && py < b)
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut edges = Vec::<Edge>::new();
    let mut outgoing = BTreeMap::<(i64, i64), Vec<usize>>::new();
    let mut add = |a: Point, b: Point, direction| {
        outgoing.entry(a.key()).or_default().push(edges.len());
        edges.push(Edge {
            a,
            b,
            direction,
            used: false,
        });
    };
    for row in 0..occupied.len() {
        for col in 0..occupied[row].len() {
            if !occupied[row][col] {
                continue;
            }
            let tl = Point(xs[col], ys[row]);
            let tr = Point(xs[col + 1], ys[row]);
            let br = Point(xs[col + 1], ys[row + 1]);
            let bl = Point(xs[col], ys[row + 1]);
            if row == 0 || !occupied[row - 1][col] {
                add(tl, tr, 0);
            }
            if !occupied[row].get(col + 1).copied().unwrap_or(false) {
                add(tr, br, 1);
            }
            if row + 1 == occupied.len() || !occupied[row + 1][col] {
                add(br, bl, 2);
            }
            if col == 0 || !occupied[row][col - 1] {
                add(bl, tl, 3);
            }
        }
    }
    let mut loops = Vec::<Vec<Point>>::new();
    for first in 0..edges.len() {
        if edges[first].used {
            continue;
        }
        let mut edge = first;
        let mut points = Vec::new();
        loop {
            edges[edge].used = true;
            points.push(edges[edge].a);
            if edges[edge].b.key() == edges[first].a.key() {
                break;
            }
            let choices = outgoing
                .get(&edges[edge].b.key())
                .ok_or("Unclosed frame contour")?;
            edge = *choices
                .iter()
                .filter(|&&i| !edges[i].used)
                .min_by_key(|&&i| {
                    let turn = (edges[i].direction + 4 - edges[edge].direction) % 4;
                    [1, 0, 3, 2].iter().position(|&t| t == turn).unwrap_or(4)
                })
                .ok_or("Unclosed frame contour")?;
            if points.len() > edges.len() {
                return Err("Unclosed frame contour".into());
            }
        }
        loops.push(
            points
                .iter()
                .enumerate()
                .filter_map(|(i, &p)| {
                    let a = points[(i + points.len() - 1) % points.len()];
                    let b = points[(i + 1) % points.len()];
                    ((p.0 - a.0) * (b.1 - p.1) != (p.1 - a.1) * (b.0 - p.0)).then_some(p)
                })
                .collect(),
        );
    }
    let body = loops
        .iter()
        .map(|p| path(p, true, width, height, radii, corner_mode, square_edges))
        .collect::<Vec<_>>()
        .join(" ");
    let mut rim = Vec::new();
    for points in &loops {
        let excluded = points
            .iter()
            .enumerate()
            .map(|(i, &p)| {
                outer(
                    p,
                    points[(i + 1) % points.len()],
                    width,
                    height,
                    square_edges,
                )
            })
            .collect::<Vec<_>>();
        if area(points) < 0. || !excluded.contains(&true) {
            rim.push(path(
                points,
                true,
                width,
                height,
                radii,
                corner_mode,
                square_edges,
            ));
            continue;
        }
        let start = excluded
            .iter()
            .position(|&e| e)
            .ok_or("Missing frame outer edge")?;
        let mut chain = Vec::new();
        for step in 1..=points.len() {
            let i = (start + step) % points.len();
            if excluded[i] {
                if chain.len() > 1 {
                    rim.push(path(
                        &chain,
                        false,
                        width,
                        height,
                        radii,
                        corner_mode,
                        square_edges,
                    ));
                }
                chain.clear();
            } else {
                if chain.is_empty() {
                    chain.push(points[i]);
                }
                chain.push(points[(i + 1) % points.len()]);
            }
        }
        if chain.len() > 1 {
            rim.push(path(
                &chain,
                false,
                width,
                height,
                radii,
                corner_mode,
                square_edges,
            ));
        }
    }
    Ok(Geometry {
        body,
        rim: rim.join(" "),
    })
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;

    #[test]
    fn reveal_samples_body_and_per_corner_radii_on_shared_progress() {
        let descriptor = RevealDescriptor {
            output_bounds: Rect::new(0., 0., 1000., 800.),
            crop: Rect::new(100., 50., 400., 300.),
            static_rects: vec![Rect::new(0., 0., 1000., 40.)],
            holes: vec![],
            animated_from: Rect::new(180., 80., 10., 12.),
            animated_to: Rect::new(120., 40., 300., 240.),
            content_clip: None,
            radii_from: [0.; 4],
            radii_to: [24., 25., 26., 27.],
            corner_mode: CornerMode::QuadraticCut,
            square_edges: 0,
            fill: [20, 30, 40, 255],
            opacity_plane: 0,
            visible: true,
            rim: None,
            shadow: None,
        };
        let sample = descriptor.sample(0.5);
        assert_eq!(sample.body, Rect::new(150., 60., 155., 126.));
        assert_eq!(sample.radii, [12., 12.5, 13., 13.5]);
        assert_eq!(descriptor.sample(-0.5).body, descriptor.animated_from);
        assert_eq!(descriptor.sample(1.5).body, descriptor.animated_to);
    }

    #[test]
    fn reveal_hit_mask_unions_static_and_animated_rects_then_applies_holes_and_clip() {
        let mut descriptor = RevealDescriptor::empty();
        descriptor.output_bounds = Rect::new(0., 0., 100., 100.);
        descriptor.crop = descriptor.output_bounds;
        descriptor.static_rects = vec![Rect::new(0., 0., 20., 100.)];
        descriptor.animated_from = Rect::new(40., 45., 40., 40.);
        descriptor.animated_to = descriptor.animated_from;
        descriptor.radii_from = [8.; 4];
        descriptor.radii_to = [8.; 4];
        descriptor.holes = vec![Rect::new(50., 50., 10., 10.)];
        descriptor.content_clip =
            Some((Rect::new(10., 10., 80., 80.), Rect::new(10., 10., 80., 80.)));
        assert!(descriptor.contains((10., 50.), 0.));
        assert!(descriptor.contains((45., 50.), 0.));
        assert!(!descriptor.contains((40.2, 45.2), 0.));
        assert!(!descriptor.contains((55., 55.), 0.));
        assert!(!descriptor.contains((95., 50.), 0.));
    }

    #[test]
    fn rounded_reveal_corners_do_not_accept_hidden_input() {
        let mut descriptor = RevealDescriptor::empty();
        descriptor.output_bounds = Rect::new(0., 0., 40., 40.);
        descriptor.crop = descriptor.output_bounds;
        descriptor.animated_from = Rect::new(4., 4., 32., 32.);
        descriptor.animated_to = descriptor.animated_from;
        descriptor.radii_from = [8.; 4];
        descriptor.radii_to = [8.; 4];
        descriptor.corner_mode = CornerMode::QuadraticCut;
        assert!(!descriptor.contains((4.5, 4.5), 0.));
        assert!(descriptor.contains((8., 8.), 0.));
    }

    #[test]
    fn rasterize_uses_bounded_crop_and_physical_scale() {
        let mut descriptor = RevealDescriptor::empty();
        descriptor.output_bounds = Rect::new(0., 0., 32., 32.);
        descriptor.crop = Rect::new(4., 4., 24., 24.);
        descriptor.animated_from = Rect::new(4., 4., 24., 24.);
        descriptor.animated_to = descriptor.animated_from;
        descriptor.radii_from = [5.; 4];
        descriptor.radii_to = [5.; 4];
        descriptor.square_edges = 0;
        descriptor.fill = [200, 100, 50, 255];
        let (one, crop) = descriptor.rasterize(0., 1., (32., 32.)).unwrap();
        assert_eq!(crop, descriptor.crop);
        assert_eq!((one.width(), one.height()), (24, 24));
        assert_eq!(one.data()[(12 * 24 + 12) * 4 + 3], 255);
        let (fractional, _) = descriptor.rasterize(0.5, 1.5, (32., 32.)).unwrap();
        assert_eq!((fractional.width(), fractional.height()), (36, 36));
        assert_eq!(fractional.data()[(18 * 36 + 18) * 4 + 3], 255);
    }

    #[test]
    fn fractional_edge_band_uses_svg_raster_ratio_after_rounding() {
        let scale = 1.5f32;
        let (a, b) = (424. / scale, 503. / scale);
        let crop = Rect::new(0., 30. + a, 1048., b - a);
        let transform = raster_transform(Rect::new(-436., -18., 1920., 1200.), crop, 1572, 79);
        let (scale_x, scale_y) = transform.get_scale();
        assert_eq!(scale_x, 1.5);
        assert_eq!(scale_y, 79. / (b - a));
        assert_ne!(scale_y, scale);
    }

    #[test]
    fn circular_raster_clamps_radii_for_tiny_and_nonuniform_geometry() {
        let mut descriptor = RevealDescriptor::empty();
        descriptor.output_bounds = Rect::new(0., 0., 20., 4.);
        descriptor.crop = descriptor.output_bounds;
        descriptor.animated_from = Rect::new(1., 1., 18., 0.75);
        descriptor.animated_to = descriptor.animated_from;
        descriptor.radii_from = [8., 12., 2., 5.];
        descriptor.radii_to = descriptor.radii_from;
        descriptor.corner_mode = CornerMode::Circular;
        descriptor.fill = [255, 255, 255, 255];
        let (pixmap, _) = descriptor.rasterize(0., 1., (20., 4.)).unwrap();
        let alpha = pixmap
            .data()
            .chunks_exact(4)
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        assert!(alpha.iter().any(|&alpha| 0 < alpha && alpha < 255));
        assert!(!alpha.contains(&255));

        descriptor.animated_from = Rect::new(1., 1., 18., 2.);
        descriptor.animated_to = descriptor.animated_from;
        let (full_height, _) = descriptor.rasterize(0., 1., (20., 4.)).unwrap();
        assert_eq!((full_height.width(), full_height.height()), (20, 4));
        assert_eq!(full_height.data()[(2 * 20 + 10) * 4 + 3], 255);
    }

    #[test]
    fn rejects_unsupported_compound_shadow_and_degenerate_rim_gradient() {
        let mut descriptor = RevealDescriptor::empty();
        descriptor.output_bounds = Rect::new(0., 0., 100., 100.);
        descriptor.crop = descriptor.output_bounds;
        descriptor.animated_from = Rect::new(10., 10., 80., 80.);
        descriptor.animated_to = descriptor.animated_from;
        descriptor.corner_mode = CornerMode::Circular;
        descriptor.shadow = Some(ShadowStyle {
            color: [0, 0, 0, 128],
            offset: (3., 4.),
            blur_radius: 8.,
            spread: 2.,
        });
        assert!(descriptor.is_valid());
        descriptor.static_rects.push(Rect::new(0., 0., 100., 10.));
        assert!(!descriptor.is_valid());
        descriptor.static_rects.clear();
        descriptor.shadow = None;
        descriptor.rim = Some(RimStyle {
            colors: [[255; 4]; 3],
            stops: [0., 0.5, 1.],
            start: (12., 12.),
            end: (12., 12.),
            width: 1.,
            opacity: 1.,
        });
        assert!(!descriptor.is_valid());
    }
}
