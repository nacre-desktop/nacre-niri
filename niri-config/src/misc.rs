use crate::appearance::{Color, WorkspaceShadow, WorkspaceShadowPart, DEFAULT_BACKDROP_COLOR};
use crate::utils::{Flag, MergeWith};
use crate::FloatOrInt;

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct SpawnAtStartup {
    #[knuffel(arguments)]
    pub command: Vec<String>,
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct SpawnShAtStartup {
    #[knuffel(argument)]
    pub command: String,
}

#[derive(Debug, PartialEq)]
pub struct Cursor {
    pub xcursor_theme: String,
    pub xcursor_size: u8,
    pub hide_when_typing: bool,
    pub hide_after_inactive_ms: Option<u32>,
    pub shake_to_enlarge: bool,
}

impl Default for Cursor {
    fn default() -> Self {
        Self {
            xcursor_theme: String::from("default"),
            xcursor_size: 24,
            hide_when_typing: false,
            hide_after_inactive_ms: None,
            shake_to_enlarge: false,
        }
    }
}

#[derive(knuffel::Decode, Debug, PartialEq)]
pub struct CursorPart {
    #[knuffel(child, unwrap(argument))]
    pub xcursor_theme: Option<String>,
    #[knuffel(child, unwrap(argument))]
    pub xcursor_size: Option<u8>,
    #[knuffel(child)]
    pub hide_when_typing: Option<Flag>,
    #[knuffel(child, unwrap(argument))]
    pub hide_after_inactive_ms: Option<u32>,
    #[knuffel(child)]
    pub shake_to_enlarge: Option<Flag>,
}

impl MergeWith<CursorPart> for Cursor {
    fn merge_with(&mut self, part: &CursorPart) {
        merge_clone!((self, part), xcursor_theme, xcursor_size);
        merge!((self, part), hide_when_typing);
        merge_clone_opt!((self, part), hide_after_inactive_ms);
        merge!((self, part), shake_to_enlarge);
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct ScreenshotPath(#[knuffel(argument)] pub Option<String>);

impl Default for ScreenshotPath {
    fn default() -> Self {
        Self(Some(String::from(
            "~/Pictures/Screenshots/Screenshot from %Y-%m-%d %H-%M-%S.png",
        )))
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyOverlay {
    pub skip_at_startup: bool,
    pub hide_not_bound: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct HotkeyOverlayPart {
    #[knuffel(child)]
    pub skip_at_startup: Option<Flag>,
    #[knuffel(child)]
    pub hide_not_bound: Option<Flag>,
}

impl MergeWith<HotkeyOverlayPart> for HotkeyOverlay {
    fn merge_with(&mut self, part: &HotkeyOverlayPart) {
        merge!((self, part), skip_at_startup, hide_not_bound);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfigNotification {
    pub disable_failed: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ConfigNotificationPart {
    #[knuffel(child)]
    pub disable_failed: Option<Flag>,
}

impl MergeWith<ConfigNotificationPart> for ConfigNotification {
    fn merge_with(&mut self, part: &ConfigNotificationPart) {
        merge!((self, part), disable_failed);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Clipboard {
    pub disable_primary: bool,
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardPart {
    #[knuffel(child)]
    pub disable_primary: Option<Flag>,
}

impl MergeWith<ClipboardPart> for Clipboard {
    fn merge_with(&mut self, part: &ClipboardPart) {
        merge!((self, part), disable_primary);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overview {
    pub zoom: f64,
    pub backdrop_color: Color,
    pub workspace_shadow: WorkspaceShadow,
}

impl Default for Overview {
    fn default() -> Self {
        Self {
            zoom: 0.5,
            backdrop_color: DEFAULT_BACKDROP_COLOR,
            workspace_shadow: WorkspaceShadow::default(),
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, Copy, PartialEq)]
pub struct OverviewPart {
    #[knuffel(child, unwrap(argument))]
    pub zoom: Option<FloatOrInt<0, 1>>,
    #[knuffel(child)]
    pub backdrop_color: Option<Color>,
    #[knuffel(child)]
    pub workspace_shadow: Option<WorkspaceShadowPart>,
}

impl MergeWith<OverviewPart> for Overview {
    fn merge_with(&mut self, part: &OverviewPart) {
        merge!((self, part), zoom, workspace_shadow);
        merge_clone!((self, part), backdrop_color);
    }
}

/// Nacre-specific native integrations that can be disabled for upstream behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nacre {
    pub overview_hover: bool,
    pub scratchpad: bool,
    pub window_pinning: bool,
    pub proportional_x11_widths: bool,
    pub edge_triggers: bool,
    pub window_restore: bool,
    pub force_close: bool,
    pub monitor_workspace_actions: bool,
}

impl Default for Nacre {
    fn default() -> Self {
        Self {
            overview_hover: true,
            scratchpad: true,
            window_pinning: true,
            proportional_x11_widths: true,
            edge_triggers: true,
            force_close: true,
            monitor_workspace_actions: true,
            window_restore: true,
        }
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct NacrePart {
    #[knuffel(child)]
    pub overview_hover: Option<Flag>,
    #[knuffel(child)]
    pub scratchpad: Option<Flag>,
    #[knuffel(child)]
    pub window_pinning: Option<Flag>,
    #[knuffel(child)]
    pub proportional_x11_widths: Option<Flag>,
    #[knuffel(child)]
    pub edge_triggers: Option<Flag>,
    #[knuffel(child)]
    pub window_restore: Option<Flag>,
    #[knuffel(child)]
    pub force_close: Option<Flag>,
    #[knuffel(child)]
    pub monitor_workspace_actions: Option<Flag>,
}

impl MergeWith<NacrePart> for Nacre {
    fn merge_with(&mut self, part: &NacrePart) {
        merge!(
            (self, part),
            overview_hover,
            scratchpad,
            window_pinning,
            proportional_x11_widths,
            edge_triggers,
            force_close,
            monitor_workspace_actions,
            window_restore
        );
    }
}

/// Action performed when the pointer dwells on a configured output edge.
#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct EdgeTriggerAction {
    /// Emit an `EdgeTriggered` event on the dedicated IPC stream.
    #[knuffel(child)]
    pub event: bool,
    /// Spawn a command.
    #[knuffel(child, unwrap(arguments))]
    pub spawn: Option<Vec<String>>,
    /// Spawn a shell command.
    #[knuffel(child, unwrap(argument))]
    pub spawn_sh: Option<String>,
}

/// Behavior configured for one screen edge.
#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct EdgeTrigger {
    /// Disable this edge, including an inherited global edge on an output override.
    #[knuffel(child)]
    pub off: bool,
    /// Optional action; defaults to emitting an IPC event.
    #[knuffel(child)]
    pub action: Option<EdgeTriggerAction>,
    /// Time the pointer must remain in the edge zone, in milliseconds.
    #[knuffel(child, unwrap(argument))]
    pub dwell_time_ms: Option<u16>,
    /// Logical pixels pushed into the edge while clipped there before triggering.
    #[knuffel(child, unwrap(argument))]
    pub pressure: Option<FloatOrInt<0, 1_000_000>>,
    /// Minimum inward pointer speed, in logical pixels per second. Zero disables the gate.
    #[knuffel(child, unwrap(argument))]
    pub velocity: Option<FloatOrInt<0, 1_000_000>>,
    /// Logical pixels inward from the output edge that form the trigger band.
    /// Defaults to one pixel to preserve the native edge trigger behavior.
    #[knuffel(child, unwrap(argument))]
    pub trigger_distance: Option<FloatOrInt<1, 10_000>>,
    /// Centered span along the output edge that accepts the trigger; omitted means the full edge.
    #[knuffel(child, unwrap(argument))]
    pub trigger_length: Option<FloatOrInt<1, 1_000_000>>,
}

/// Sticky inner monitor edges: the pointer is held at an edge shared with another output while
/// that edge's trigger runs, unless the push is fast or long enough to cross. On by default.
#[derive(knuffel::Decode, Debug, Clone, PartialEq)]
pub struct InnerEdgePush {
    #[knuffel(child)]
    pub off: bool,
    /// Hold against the edge before the trigger fires; replaces `dwell-time-ms` on inner edges.
    #[knuffel(child, unwrap(argument), default = 180)]
    pub hold_time_ms: u16,
    /// Inward pointer speed, in logical pixels per second, that crosses immediately.
    #[knuffel(child, unwrap(argument), default = FloatOrInt(1500.))]
    pub cross_velocity: FloatOrInt<0, 1_000_000>,
    /// Accumulated inward push, in logical pixels, that crosses.
    #[knuffel(child, unwrap(argument), default = FloatOrInt(64.))]
    pub cross_distance: FloatOrInt<1, 1_000_000>,
    #[knuffel(child, unwrap(argument), default = true)]
    pub left: bool,
    #[knuffel(child, unwrap(argument), default = true)]
    pub right: bool,
    #[knuffel(child, unwrap(argument), default = true)]
    pub top: bool,
    #[knuffel(child, unwrap(argument), default = true)]
    pub bottom: bool,
}

impl Default for InnerEdgePush {
    fn default() -> Self {
        Self {
            off: false,
            hold_time_ms: 180,
            cross_velocity: FloatOrInt(1500.),
            cross_distance: FloatOrInt(64.),
            left: true,
            right: true,
            top: true,
            bottom: true,
        }
    }
}

/// Global edge trigger policy. Output blocks may override individual edges.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct EdgeTriggers {
    pub cue: bool,
    pub inner_edge_push: InnerEdgePush,
    pub left: Option<EdgeTrigger>,
    pub right: Option<EdgeTrigger>,
    pub top: Option<EdgeTrigger>,
    pub bottom: Option<EdgeTrigger>,
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq)]
pub struct EdgeTriggersPart {
    #[knuffel(child)]
    pub cue: Option<Flag>,
    #[knuffel(child)]
    pub inner_edge_push: Option<InnerEdgePush>,
    #[knuffel(child)]
    pub left: Option<EdgeTrigger>,
    #[knuffel(child)]
    pub right: Option<EdgeTrigger>,
    #[knuffel(child)]
    pub top: Option<EdgeTrigger>,
    #[knuffel(child)]
    pub bottom: Option<EdgeTrigger>,
}

impl MergeWith<EdgeTriggersPart> for EdgeTriggers {
    fn merge_with(&mut self, part: &EdgeTriggersPart) {
        if let Some(cue) = part.cue {
            self.cue = cue.0;
        }
        if let Some(push) = &part.inner_edge_push {
            self.inner_edge_push = push.clone();
        }
        merge_clone_opt!((self, part), left, right, top, bottom);
    }
}

#[derive(knuffel::Decode, Debug, Default, Clone, PartialEq, Eq)]
pub struct Environment(#[knuffel(children)] pub Vec<EnvironmentVariable>);

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct EnvironmentVariable {
    #[knuffel(node_name)]
    pub name: String,
    #[knuffel(argument)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSatellite {
    pub off: bool,
    pub path: String,
}

impl Default for XwaylandSatellite {
    fn default() -> Self {
        Self {
            off: false,
            path: String::from("xwayland-satellite"),
        }
    }
}

#[derive(knuffel::Decode, Debug, Clone, PartialEq, Eq)]
pub struct XwaylandSatellitePart {
    #[knuffel(child)]
    pub off: bool,
    #[knuffel(child)]
    pub on: bool,
    #[knuffel(child, unwrap(argument))]
    pub path: Option<String>,
}

impl MergeWith<XwaylandSatellitePart> for XwaylandSatellite {
    fn merge_with(&mut self, part: &XwaylandSatellitePart) {
        self.off |= part.off;
        if part.on {
            self.off = false;
        }

        merge_clone!((self, part), path);
    }
}
