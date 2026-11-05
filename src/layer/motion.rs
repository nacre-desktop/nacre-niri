//! Presentation-time sampling for compositor-driven layer motion.

use std::time::Duration;

use smithay::utils::{Logical, Point, Rectangle, Size};

/// The complete presentation state for one animated layer plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionPose {
    /// Destination rectangle in layer-surface logical coordinates.
    pub destination: Rectangle<f64, Logical>,
    /// Fixed-frame reveal rectangle in layer-surface logical coordinates.
    pub clip: Rectangle<f64, Logical>,
    /// Plane opacity in the inclusive range 0..=1.
    pub opacity: f64,
}

impl MotionPose {
    const MAX_COORDINATE: f64 = 32_768.;

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        clip_x: f64,
        clip_y: f64,
        clip_width: f64,
        clip_height: f64,
        opacity: f64,
    ) -> Option<Self> {
        let pose = Self {
            destination: Rectangle::new(Point::from((x, y)), Size::from((width, height))),
            clip: Rectangle::new(
                Point::from((clip_x, clip_y)),
                Size::from((clip_width, clip_height)),
            ),
            opacity,
        };
        pose.is_valid().then_some(pose)
    }

    pub fn is_valid(&self) -> bool {
        let values = [
            self.destination.loc.x,
            self.destination.loc.y,
            self.destination.size.w,
            self.destination.size.h,
            self.clip.loc.x,
            self.clip.loc.y,
            self.clip.size.w,
            self.clip.size.h,
            self.opacity,
        ];
        values.into_iter().all(f64::is_finite)
            && self.destination.loc.x.abs() <= Self::MAX_COORDINATE
            && self.destination.loc.y.abs() <= Self::MAX_COORDINATE
            && self.destination.size.w <= Self::MAX_COORDINATE
            && self.destination.size.h <= Self::MAX_COORDINATE
            && self.clip.loc.x.abs() <= Self::MAX_COORDINATE
            && self.clip.loc.y.abs() <= Self::MAX_COORDINATE
            && self.clip.size.w <= Self::MAX_COORDINATE
            && self.clip.size.h <= Self::MAX_COORDINATE
            && self.destination.size.w >= 0.
            && self.destination.size.h >= 0.
            && self.clip.size.w >= 0.
            && self.clip.size.h >= 0.
            && (0. ..=1.).contains(&self.opacity)
    }

    pub fn map_input(
        self,
        point: Point<f64, Logical>,
        source: Rectangle<f64, Logical>,
    ) -> Option<Point<f64, Logical>> {
        if self.opacity <= 0.
            || self.destination.size.w <= 0.
            || self.destination.size.h <= 0.
            || !self.clip.contains(point)
            || !self.destination.contains(point)
        {
            return None;
        }
        let x = (point.x - self.destination.loc.x) / self.destination.size.w;
        let y = (point.y - self.destination.loc.y) / self.destination.size.h;
        Some(Point::from((
            source.loc.x + x * source.size.w,
            source.loc.y + y * source.size.h,
        )))
    }

    fn interpolate(self, to: Self, progress: f64, opacity_mode: OpacityMode) -> Self {
        let lerp = |a: f64, b: f64| a + (b - a) * progress;
        Self {
            destination: Rectangle::new(
                Point::from((
                    lerp(self.destination.loc.x, to.destination.loc.x),
                    lerp(self.destination.loc.y, to.destination.loc.y),
                )),
                Size::from((
                    lerp(self.destination.size.w, to.destination.size.w),
                    lerp(self.destination.size.h, to.destination.size.h),
                )),
            ),
            clip: Rectangle::new(
                Point::from((
                    lerp(self.clip.loc.x, to.clip.loc.x),
                    lerp(self.clip.loc.y, to.clip.loc.y),
                )),
                Size::from((
                    lerp(self.clip.size.w, to.clip.size.w),
                    lerp(self.clip.size.h, to.clip.size.h),
                )),
            ),
            opacity: match opacity_mode {
                OpacityMode::Linear => lerp(self.opacity, to.opacity),
                OpacityMode::OpaqueAbove(threshold) => {
                    let opacity = lerp(self.opacity, to.opacity);
                    if opacity > threshold as f64 / 1_000_000. {
                        1.
                    } else {
                        opacity
                    }
                }
            },
        }
    }
}

/// Curves shared with Bead's framework motion tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MotionCurve {
    Linear,
    EaseOutCubic,
    EaseInOutCubic,
    EmphasizedAccelerate,
    CriticalSpring,
}

/// Post-interpolation opacity behavior for a plane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpacityMode {
    Linear,
    OpaqueAbove(u32),
}

impl OpacityMode {
    fn is_valid(self) -> bool {
        match self {
            Self::Linear => true,
            Self::OpaqueAbove(threshold) => threshold <= 1_000_000,
        }
    }
}

impl MotionCurve {
    pub fn sample(self, t: f64) -> f64 {
        let t = t.clamp(0., 1.);
        match self {
            Self::Linear => t,
            Self::EaseOutCubic => 1. - (1. - t).powi(3),
            Self::EaseInOutCubic if t < 0.5 => 4. * t.powi(3),
            Self::EaseInOutCubic => 1. - (-2. * t + 2.).powi(3) / 2.,
            Self::EmphasizedAccelerate => t.powi(4),
            Self::CriticalSpring => {
                let terminal = 9. * (-8_f64).exp();
                (1. - (1. + 8. * t) * (-8. * t).exp()) / (1. - terminal)
            }
        }
    }
}

/// A time-based interpolation sampled using the output's predicted presentation time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayerMotion {
    pub serial: u32,
    from: MotionPose,
    to: MotionPose,
    start: Duration,
    duration: Duration,
    curve: MotionCurve,
    opacity_mode: OpacityMode,
}

/// One atomically animated set of atlas planes sharing a single timeline.
#[derive(Clone, Debug, PartialEq)]
pub struct LayerMotionGroup {
    pub serial: u32,
    from: Vec<(u32, MotionPose)>,
    to: Vec<(u32, MotionPose)>,
    opacity_modes: Vec<(u32, OpacityMode)>,
    start: Duration,
    duration: Duration,
    curve: MotionCurve,
}

impl LayerMotionGroup {
    pub fn new(
        serial: u32,
        from: Vec<(u32, MotionPose)>,
        to: Vec<(u32, MotionPose)>,
        opacity_modes: Vec<(u32, OpacityMode)>,
        start: Duration,
        duration: Duration,
        curve: MotionCurve,
    ) -> Option<Self> {
        if from.is_empty()
            || from.len() != to.len()
            || from.len() != opacity_modes.len()
            || from.iter().zip(&to).any(|((from_id, from), (to_id, to))| {
                from_id != to_id || !from.is_valid() || !to.is_valid()
            })
            || from
                .iter()
                .zip(&opacity_modes)
                .any(|((id, _), (mode_id, mode))| id != mode_id || !mode.is_valid())
            || duration.is_zero()
            || duration > LayerMotion::MAX_DURATION
        {
            return None;
        }
        Some(Self {
            serial,
            from,
            to,
            opacity_modes,
            start,
            duration,
            curve,
        })
    }

    pub fn sample(&self, presentation_time: Duration) -> Vec<(u32, MotionPose)> {
        let p = self.progress(presentation_time);
        self.from
            .iter()
            .zip(&self.to)
            .zip(&self.opacity_modes)
            .map(|(((id, from), (_, to)), (_, opacity_mode))| {
                (*id, from.interpolate(*to, p, *opacity_mode))
            })
            .collect()
    }

    /// Eased progress shared by every animated field attached to this group.
    pub fn progress(&self, presentation_time: Duration) -> f64 {
        let elapsed = presentation_time.saturating_sub(self.start);
        self.curve
            .sample(elapsed.as_secs_f64() / self.duration.as_secs_f64())
    }

    pub fn is_complete(&self, presentation_time: Duration) -> bool {
        presentation_time >= self.start.saturating_add(self.duration)
    }

    pub fn start_time(&self) -> Duration {
        self.start
    }

    pub fn retarget(
        &self,
        serial: u32,
        to: Vec<(u32, MotionPose)>,
        opacity_modes: Vec<(u32, OpacityMode)>,
        presentation_time: Duration,
        duration: Duration,
        curve: MotionCurve,
    ) -> Option<Self> {
        Self::new(
            serial,
            self.sample(presentation_time),
            to,
            opacity_modes,
            presentation_time,
            duration,
            curve,
        )
    }

    pub fn final_poses(&self) -> Vec<(u32, MotionPose)> {
        self.from
            .iter()
            .zip(&self.to)
            .zip(&self.opacity_modes)
            .map(|(((id, from), (_, to)), (_, mode))| (*id, from.interpolate(*to, 1., *mode)))
            .collect()
    }
}

impl LayerMotion {
    pub const MAX_DURATION: Duration = Duration::from_secs(10);

    pub fn new(
        serial: u32,
        from: MotionPose,
        to: MotionPose,
        start: Duration,
        duration: Duration,
        curve: MotionCurve,
        opacity_mode: OpacityMode,
    ) -> Option<Self> {
        if !from.is_valid()
            || !to.is_valid()
            || duration.is_zero()
            || duration > Self::MAX_DURATION
            || !opacity_mode.is_valid()
        {
            return None;
        }
        Some(Self {
            serial,
            from,
            to,
            start,
            duration,
            curve,
            opacity_mode,
        })
    }

    pub fn sample(&self, presentation_time: Duration) -> MotionPose {
        let elapsed = presentation_time.saturating_sub(self.start);
        let t = elapsed.as_secs_f64() / self.duration.as_secs_f64();
        self.from
            .interpolate(self.to, self.curve.sample(t), self.opacity_mode)
    }

    pub fn is_complete(&self, presentation_time: Duration) -> bool {
        presentation_time >= self.start.saturating_add(self.duration)
    }

    /// Retarget from the state currently presented at `presentation_time`.
    pub fn retarget(
        &self,
        serial: u32,
        to: MotionPose,
        presentation_time: Duration,
        duration: Duration,
        curve: MotionCurve,
        opacity_mode: OpacityMode,
    ) -> Option<Self> {
        Self::new(
            serial,
            self.sample(presentation_time),
            to,
            presentation_time,
            duration,
            curve,
            opacity_mode,
        )
    }

    pub fn final_pose(&self) -> MotionPose {
        self.from.interpolate(self.to, 1., self.opacity_mode)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn pose(x: f64, width: f64, opacity: f64) -> MotionPose {
        MotionPose {
            destination: Rectangle::new(Point::from((x, 0.)), Size::from((width, 20.))),
            clip: Rectangle::new(Point::from((0., 0.)), Size::from((width, 20.))),
            opacity,
        }
    }

    proptest! {
        #[test]
        fn randomized_motion_sampling_stays_valid_and_retarget_is_continuous(
            from_x in -20_000.0f64..20_000.0,
            from_y in -20_000.0f64..20_000.0,
            from_w in 0.0f64..10_000.0,
            from_h in 0.0f64..10_000.0,
            from_opacity in 0.0f64..=1.0,
            to_x in -20_000.0f64..20_000.0,
            to_y in -20_000.0f64..20_000.0,
            to_w in 0.0f64..10_000.0,
            to_h in 0.0f64..10_000.0,
            to_opacity in 0.0f64..=1.0,
            curve_index in 0u8..5,
            elapsed_ms in 0u64..10_000,
        ) {
            let from = MotionPose::new(
                from_x, from_y, from_w, from_h, from_x, from_y, from_w, from_h, from_opacity,
            ).unwrap();
            let to = MotionPose::new(
                to_x, to_y, to_w, to_h, to_x, to_y, to_w, to_h, to_opacity,
            ).unwrap();
            let curve = match curve_index {
                0 => MotionCurve::Linear,
                1 => MotionCurve::EaseOutCubic,
                2 => MotionCurve::EaseInOutCubic,
                3 => MotionCurve::EmphasizedAccelerate,
                _ => MotionCurve::CriticalSpring,
            };
            let duration = Duration::from_millis(10_000);
            let motion = LayerMotion::new(
                1, from, to, Duration::ZERO, duration, curve, OpacityMode::Linear,
            ).unwrap();
            let sampled = motion.sample(Duration::from_millis(elapsed_ms));
            prop_assert!(sampled.is_valid());
            let retargeted = motion.retarget(
                2, from, Duration::from_millis(elapsed_ms), duration, curve, OpacityMode::Linear,
            ).unwrap();
            prop_assert_eq!(retargeted.sample(Duration::from_millis(elapsed_ms)), sampled);
        }

        #[test]
        fn randomized_visible_plane_input_maps_back_to_source(
            x in -20_000.0f64..20_000.0,
            y in -20_000.0f64..20_000.0,
            width in 1.0f64..10_000.0,
            height in 1.0f64..10_000.0,
            fx in 0.01f64..0.99,
            fy in 0.01f64..0.99,
            opacity in 0.01f64..=1.0,
            source_x in -20_000.0f64..20_000.0,
            source_y in -20_000.0f64..20_000.0,
            source_w in 1.0f64..10_000.0,
            source_h in 1.0f64..10_000.0,
        ) {
            let plane = MotionPose::new(x, y, width, height, x, y, width, height, opacity).unwrap();
            let source = Rectangle::new(
                Point::from((source_x, source_y)),
                Size::from((source_w, source_h)),
            );
            let point = Point::from((x + fx * width, y + fy * height));
            let mapped = plane.map_input(point, source).unwrap();
            prop_assert!((mapped.x - (source_x + fx * source_w)).abs() < 1e-7);
            prop_assert!((mapped.y - (source_y + fy * source_h)).abs() < 1e-7);
        }
    }

    #[test]
    fn curves_have_expected_endpoints_and_samples() {
        for curve in [
            MotionCurve::Linear,
            MotionCurve::EaseOutCubic,
            MotionCurve::EaseInOutCubic,
            MotionCurve::EmphasizedAccelerate,
            MotionCurve::CriticalSpring,
        ] {
            assert!((curve.sample(0.)).abs() < 1e-12);
            assert!((curve.sample(1.) - 1.).abs() < 1e-12);
        }
        assert!((MotionCurve::EaseOutCubic.sample(0.5) - 0.875).abs() < 1e-12);
        assert!((MotionCurve::EaseInOutCubic.sample(0.5) - 0.5).abs() < 1e-12);
        assert!((MotionCurve::EmphasizedAccelerate.sample(0.5) - 0.0625).abs() < 1e-12);
    }

    #[test]
    fn sampled_pose_interpolates_destination_clip_and_opacity() {
        let motion = LayerMotion::new(
            7,
            pose(-100., 0., 0.),
            pose(0., 200., 1.),
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
            OpacityMode::Linear,
        )
        .unwrap();
        let mid = motion.sample(Duration::from_millis(50));
        assert_eq!(mid.destination.loc.x, -50.);
        assert_eq!(mid.destination.size.w, 100.);
        assert_eq!(mid.clip.size.w, 100.);
        assert_eq!(mid.opacity, 0.5);
        assert_eq!(
            motion.sample(Duration::from_millis(100)),
            pose(0., 200., 1.)
        );
    }

    #[test]
    fn retarget_starts_at_the_currently_presented_pose() {
        let first = LayerMotion::new(
            1,
            pose(-100., 0., 0.),
            pose(0., 200., 1.),
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
            OpacityMode::Linear,
        )
        .unwrap();
        let at_retarget = Duration::from_millis(40);
        let sampled = first.sample(at_retarget);
        let reversed = first
            .retarget(
                2,
                pose(-100., 0., 0.),
                at_retarget,
                Duration::from_millis(60),
                MotionCurve::Linear,
                OpacityMode::Linear,
            )
            .unwrap();
        assert_eq!(reversed.sample(at_retarget), sampled);
        assert_eq!(reversed.serial, 2);
    }

    #[test]
    fn rejects_invalid_pose_and_unbounded_duration() {
        assert!(LayerMotion::new(
            1,
            pose(0., 1., 1.5),
            pose(0., 1., 1.),
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
            OpacityMode::Linear,
        )
        .is_none());
        assert!(LayerMotion::new(
            1,
            pose(0., 1., 1.),
            pose(0., 1., 1.),
            Duration::ZERO,
            Duration::from_secs(11),
            MotionCurve::Linear,
            OpacityMode::Linear,
        )
        .is_none());
    }

    #[test]
    fn group_samples_and_retargets_all_planes_on_one_timeline() {
        let group = LayerMotionGroup::new(
            1,
            vec![(4, pose(-100., 0., 0.)), (8, pose(0., 20., 0.5))],
            vec![(4, pose(0., 200., 1.)), (8, pose(10., 40., 1.))],
            vec![
                (4, OpacityMode::Linear),
                (8, OpacityMode::OpaqueAbove(1_000)),
            ],
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
        )
        .unwrap();
        let at_retarget = Duration::from_millis(40);
        let current = group.sample(at_retarget);
        assert_eq!(
            current.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            [4, 8]
        );
        let target = vec![(4, pose(-100., 0., 0.)), (8, pose(0., 20., 0.5))];
        let reversed = group
            .retarget(
                2,
                target,
                vec![
                    (4, OpacityMode::Linear),
                    (8, OpacityMode::OpaqueAbove(1_000)),
                ],
                at_retarget,
                Duration::from_millis(60),
                MotionCurve::Linear,
            )
            .unwrap();
        assert_eq!(reversed.sample(at_retarget), current);
        assert_eq!(reversed.final_poses().len(), 2);
        assert_eq!(reversed.final_poses()[1].1.opacity, 1.);
    }

    #[test]
    fn opacity_threshold_preserves_step_visibility_without_a_fade() {
        let motion = LayerMotion::new(
            3,
            pose(0., 20., 0.),
            pose(0., 20., 1.),
            Duration::ZERO,
            Duration::from_millis(100),
            MotionCurve::Linear,
            OpacityMode::OpaqueAbove(1_000),
        )
        .unwrap();
        assert_eq!(motion.sample(Duration::from_micros(100)).opacity, 0.001);
        assert_eq!(motion.sample(Duration::from_micros(101)).opacity, 1.);
    }

    #[test]
    fn input_maps_through_destination_and_respects_animated_clip() {
        let source = Rectangle::new(Point::from((20., 10.)), Size::from((80., 40.)));
        let visible = MotionPose::new(100., 50., 160., 80., 120., 50., 140., 80., 1.).unwrap();
        assert_eq!(
            visible.map_input(Point::from((200., 90.)), source),
            Some(Point::from((70., 30.)))
        );
        assert!(visible
            .map_input(Point::from((110., 90.)), source)
            .is_none());
        let hidden = MotionPose::new(100., 50., 160., 80., 100., 50., 160., 80., 0.).unwrap();
        assert!(hidden.map_input(Point::from((150., 80.)), source).is_none());
    }
}
