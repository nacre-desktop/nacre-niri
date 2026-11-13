//! Bounded launch-to-window workspace affinity.

use std::time::{Duration, Instant};

use crate::launch_process::ProcessIdentity;
use crate::layout::workspace::WorkspaceId;

/// Launch intents expire at this boundary.
pub const LAUNCH_WORKSPACE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PENDING_LAUNCHES: usize = 128;
const MAX_FOLLOWER_APP_ID_BYTES: usize = 255;

pub fn launch_is_off_view(target: Option<WorkspaceId>, active: Option<WorkspaceId>) -> bool {
    target.is_some() && target != active
}

pub fn suppress_late_activation(
    consumed: bool,
    target: Option<WorkspaceId>,
    active: Option<WorkspaceId>,
) -> bool {
    consumed && launch_is_off_view(target, active)
}

pub fn launch_tokens_from_environ(environ: &[u8]) -> impl Iterator<Item = &str> {
    environ.split(|byte| *byte == 0).filter_map(|entry| {
        [
            b"XDG_ACTIVATION_TOKEN=".as_slice(),
            b"DESKTOP_STARTUP_ID=".as_slice(),
        ]
        .iter()
        .find_map(|prefix| {
            if !entry.starts_with(prefix) {
                return None;
            }
            let value = &entry[prefix.len()..];
            let value = std::str::from_utf8(value).ok()?;
            (!value.is_empty()).then_some(value)
        })
    })
}

#[derive(Debug, Clone)]
pub struct LaunchIntent {
    pub token: String,
    pub unit: Option<String>,
    pub process: Option<ProcessIdentity>,
    pub workspace: WorkspaceId,
    pub created: Instant,
    consumed: bool,
    follower: Option<LaunchFollower>,
}

#[derive(Debug, Clone)]
struct LaunchFollower {
    process: Option<ProcessIdentity>,
    first_consumer_process: Option<ProcessIdentity>,
    app_id: Option<String>,
    first_window: Option<crate::window::mapped::MappedId>,
    window_workspace: Option<WorkspaceId>,
}

#[derive(Debug, Default)]
pub struct LaunchTracker {
    pending: Vec<LaunchIntent>,
}

impl LaunchTracker {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn has_followers(&mut self, now: Instant) -> bool {
        self.expire(now);
        self.pending.iter().any(|intent| intent.follower.is_some())
    }

    pub fn contains_token(&mut self, token: &str, now: Instant) -> bool {
        self.expire(now);
        self.pending.iter().any(|intent| intent.token == token)
    }

    pub fn prepare(
        &mut self,
        token: String,
        unit: Option<String>,
        workspace: WorkspaceId,
        now: Instant,
    ) {
        self.expire(now);
        if let Some(existing) = self.pending.iter_mut().find(|intent| intent.token == token) {
            if !existing.consumed {
                existing.unit = unit.or_else(|| existing.unit.clone());
            }
            return;
        }
        if self.pending.len() == MAX_PENDING_LAUNCHES {
            self.pending.remove(0);
        }
        self.pending.push(LaunchIntent {
            token,
            unit,
            process: None,
            workspace,
            created: now,
            consumed: false,
            follower: None,
        });
    }

    pub fn bind_unit(&mut self, token: &str, unit: String, now: Instant) -> bool {
        self.expire(now);
        if let Some(intent) = self
            .pending
            .iter_mut()
            .find(|intent| intent.token == token && !intent.consumed)
        {
            intent.unit = Some(unit);
            true
        } else {
            false
        }
    }

    pub fn take_token(&mut self, token: &str, now: Instant) -> Option<LaunchIntent> {
        self.expire(now);
        let intent = self
            .pending
            .iter_mut()
            .find(|intent| intent.token == token && !intent.consumed)?;
        intent.consumed = true;
        Some(intent.clone())
    }

    pub fn take_unit(&mut self, unit: &str, now: Instant) -> Option<LaunchIntent> {
        self.expire(now);
        let intent = self
            .pending
            .iter_mut()
            .find(|intent| !intent.consumed && intent.unit.as_deref() == Some(unit))?;
        intent.consumed = true;
        Some(intent.clone())
    }

    pub fn peek_token(&mut self, token: &str, now: Instant) -> Option<LaunchIntent> {
        self.expire(now);
        self.pending
            .iter()
            .find(|intent| intent.token == token && !intent.consumed)
            .cloned()
    }

    pub fn peek_unit(&mut self, unit: &str, now: Instant) -> Option<LaunchIntent> {
        self.expire(now);
        self.pending
            .iter()
            .find(|intent| !intent.consumed && intent.unit.as_deref() == Some(unit))
            .cloned()
    }

    pub fn bind_process(&mut self, token: &str, process: ProcessIdentity, now: Instant) -> bool {
        self.expire(now);
        if let Some(intent) = self
            .pending
            .iter_mut()
            .find(|intent| intent.token == token && !intent.consumed)
        {
            intent.process = Some(process);
            true
        } else {
            false
        }
    }

    pub fn set_follower(
        &mut self,
        token: &str,
        process: Option<ProcessIdentity>,
        app_id: Option<String>,
        now: Instant,
    ) -> bool {
        self.expire(now);
        let Some(intent) = self
            .pending
            .iter_mut()
            .find(|intent| intent.token == token && intent.consumed)
        else {
            return false;
        };
        let app_id =
            app_id.filter(|app_id| !app_id.is_empty() && app_id.len() <= MAX_FOLLOWER_APP_ID_BYTES);
        if process.is_none() && intent.process.is_none() && app_id.is_none() {
            return false;
        }
        intent.follower = Some(LaunchFollower {
            process: intent.process,
            first_consumer_process: process,
            app_id,
            first_window: None,
            window_workspace: None,
        });
        true
    }

    pub fn set_follower_window(
        &mut self,
        token: &str,
        window: crate::window::mapped::MappedId,
        workspace: Option<WorkspaceId>,
        now: Instant,
    ) -> bool {
        self.expire(now);
        let Some(follower) = self
            .pending
            .iter_mut()
            .find(|intent| intent.token == token)
            .and_then(|intent| intent.follower.as_mut())
        else {
            return false;
        };
        follower.first_window = Some(window);
        follower.window_workspace = workspace;
        true
    }

    /// Match a later toplevel to the first consumer of a still-live launch.
    /// Process ancestry wins; app-id matching is used only when unambiguous.
    pub fn peek_follower(
        &mut self,
        ancestry: &[ProcessIdentity],
        app_id: Option<&str>,
        now: Instant,
    ) -> Option<LaunchIntent> {
        self.expire(now);
        if let Some(intent) = self.pending.iter().rev().find(|intent| {
            intent.follower.as_ref().is_some_and(|follower| {
                follower
                    .process
                    .is_some_and(|process| ancestry.contains(&process))
                    || follower
                        .first_consumer_process
                        .is_some_and(|process| ancestry.contains(&process))
            })
        }) {
            return Some(intent.clone());
        }
        let app_id = app_id.filter(|app_id| !app_id.is_empty())?;
        let mut matches = self.pending.iter().filter(|intent| {
            intent
                .follower
                .as_ref()
                .and_then(|follower| follower.app_id.as_deref())
                == Some(app_id)
        });
        let intent = matches.next()?;
        matches.next().is_none().then(|| intent.clone())
    }

    pub fn follower_for_token(&mut self, token: &str, now: Instant) -> Option<LaunchIntent> {
        self.expire(now);
        self.pending
            .iter()
            .find(|intent| intent.token == token && intent.follower.is_some())
            .cloned()
    }

    pub fn follower_window(
        &mut self,
        token: &str,
        now: Instant,
    ) -> Option<crate::window::mapped::MappedId> {
        self.follower_for_token(token, now)
            .and_then(|intent| intent.follower.and_then(|follower| follower.first_window))
    }

    pub fn retire_follower_if_moved(
        &mut self,
        token: &str,
        workspace: WorkspaceId,
        now: Instant,
    ) -> bool {
        self.expire(now);
        let moved = self.pending.iter().any(|intent| {
            intent.token == token
                && intent
                    .follower
                    .as_ref()
                    .and_then(|follower| follower.window_workspace)
                    .is_some_and(|original| original != workspace)
        });
        if moved {
            self.discard_token(token, now);
        }
        moved
    }

    pub fn peek_process(
        &mut self,
        ancestry: &[ProcessIdentity],
        now: Instant,
    ) -> Option<LaunchIntent> {
        self.expire(now);
        self.pending
            .iter()
            .find(|intent| {
                !intent.consumed && intent.process.is_some_and(|root| ancestry.contains(&root))
            })
            .cloned()
    }

    pub fn discard_token(&mut self, token: &str, now: Instant) {
        self.expire(now);
        self.pending.retain(|intent| intent.token != token);
    }

    pub fn token_was_consumed(&mut self, token: &str, now: Instant) -> bool {
        self.expire(now);
        self.pending
            .iter()
            .any(|intent| intent.token == token && intent.consumed)
    }

    fn expire(&mut self, now: Instant) {
        self.pending
            .retain(|intent| now.duration_since(intent.created) < LAUNCH_WORKSPACE_TIMEOUT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expires_at_sixty_seconds_including_the_exact_boundary() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(101);
        let mut tracker = LaunchTracker::default();
        tracker.prepare(
            "t".into(),
            Some("nacre-app-a.service".into()),
            workspace,
            now,
        );
        assert!(tracker
            .peek_token(
                "t",
                now + LAUNCH_WORKSPACE_TIMEOUT - Duration::from_nanos(1)
            )
            .is_some());
        assert!(tracker
            .take_token("t", now + LAUNCH_WORKSPACE_TIMEOUT)
            .is_none());
    }

    #[test]
    fn binding_unit_keeps_original_workspace_and_clock_and_consumption_is_single_use() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(102);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("t".into(), None, workspace, now);
        assert!(tracker.bind_unit(
            "t",
            "nacre-app-a.service".into(),
            now + Duration::from_secs(20)
        ));
        let intent = tracker
            .take_unit("nacre-app-a.service", now + Duration::from_secs(59))
            .unwrap();
        assert_eq!(intent.workspace, workspace);
        assert_eq!(intent.created, now);
        assert!(tracker
            .take_token("t", now + Duration::from_secs(59))
            .is_none());
        assert!(!tracker.bind_unit(
            "t",
            "nacre-app-late.service".into(),
            now + Duration::from_secs(59)
        ));
    }

    #[test]
    fn reusing_a_token_without_a_unit_preserves_its_original_record() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(103);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("t".into(), None, workspace, now);
        let reused = tracker
            .peek_token("t", now + Duration::from_secs(20))
            .unwrap();
        assert_eq!(reused.workspace, workspace);
        assert_eq!(reused.created, now);
        assert!(tracker
            .take_token("t", now + Duration::from_secs(59))
            .is_some());
    }

    #[test]
    fn exact_workspace_service_unit_can_match_a_launch() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(104);
        let mut tracker = LaunchTracker::default();
        tracker.prepare(
            "t".into(),
            Some("nacre-workspace-notes-editor.service".into()),
            workspace,
            now,
        );
        assert_eq!(
            tracker
                .peek_unit("nacre-workspace-notes-editor.service", now)
                .unwrap()
                .workspace,
            workspace
        );
        assert!(tracker
            .peek_unit("nacre-workspace-other.service", now)
            .is_none());
    }

    #[test]
    fn pending_intents_are_bounded_and_parallel_launches_keep_distinct_workspaces() {
        let now = Instant::now();
        let mut tracker = LaunchTracker::default();
        for i in 0..MAX_PENDING_LAUNCHES {
            tracker.prepare(
                format!("token-{i}"),
                None,
                WorkspaceId::specific(i as u64),
                now,
            );
        }
        tracker.prepare("last".into(), None, WorkspaceId::specific(999), now);
        assert_eq!(tracker.pending.len(), MAX_PENDING_LAUNCHES);
        assert!(tracker.peek_token("token-0", now).is_none());
        assert_eq!(
            tracker.peek_token("token-127", now).unwrap().workspace,
            WorkspaceId::specific(127)
        );
        assert_eq!(
            tracker.peek_token("last", now).unwrap().workspace,
            WorkspaceId::specific(999)
        );
    }

    #[test]
    fn explicit_rules_and_parent_are_authoritative_and_off_view_activation_is_suppressed() {
        let a = WorkspaceId::specific(201);
        let b = WorkspaceId::specific(202);
        assert!(launch_is_off_view(Some(a), Some(b)));
        assert!(!launch_is_off_view(Some(a), Some(a)));
        assert!(suppress_late_activation(true, Some(a), Some(b)));
        assert!(!suppress_late_activation(false, Some(a), Some(b)));
    }

    #[test]
    fn extracts_only_activation_tokens_without_exposing_other_environment() {
        assert_eq!(
            launch_tokens_from_environ(
                b"PATH=/bin\0DESKTOP_STARTUP_ID=stale\0XDG_ACTIVATION_TOKEN=opaque\0OTHER=secret"
            )
            .collect::<Vec<_>>(),
            ["stale", "opaque"]
        );
        assert_eq!(
            launch_tokens_from_environ(b"DESKTOP_STARTUP_ID=\0").next(),
            None
        );
        assert_eq!(launch_tokens_from_environ(b"OTHER=opaque\0").next(), None);
    }

    #[test]
    fn process_match_requires_pid_and_start_time() {
        let now = Instant::now();
        let mut tracker = LaunchTracker::default();
        tracker.prepare("process".into(), None, WorkspaceId::specific(301), now);
        let root = ProcessIdentity {
            pid: 42,
            starttime: 100,
        };
        assert!(tracker.bind_process("process", root, now));
        assert!(tracker
            .peek_process(
                &[ProcessIdentity {
                    pid: 42,
                    starttime: 101
                }],
                now
            )
            .is_none());
        assert_eq!(tracker.peek_process(&[root], now).unwrap().token, "process");
    }

    #[test]
    fn follower_matches_pid_ancestry_even_when_app_id_changes() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(401);
        let process = ProcessIdentity {
            pid: 42,
            starttime: 100,
        };
        let mut tracker = LaunchTracker::default();
        tracker.prepare("follower".into(), None, workspace, now);
        assert!(tracker.take_token("follower", now).is_some());
        assert!(tracker.set_follower("follower", Some(process), Some("org.discord".into()), now));

        assert_eq!(
            tracker
                .peek_follower(&[process], Some("org.other"), now)
                .unwrap()
                .workspace,
            workspace
        );
    }

    #[test]
    fn app_id_only_follower_uses_its_workspace_while_other_apps_use_active_workspace() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(402);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("follower".into(), None, workspace, now);
        assert!(tracker.take_token("follower", now).is_some());
        assert!(tracker.set_follower(
            "follower",
            Some(ProcessIdentity {
                pid: 42,
                starttime: 100,
            }),
            Some("org.discord".into()),
            now
        ));

        assert_eq!(
            tracker
                .peek_follower(&[], Some("org.discord"), now)
                .unwrap()
                .workspace,
            workspace
        );
        let active_workspace = WorkspaceId::specific(407);
        let unrelated_window_workspace = tracker
            .peek_follower(&[], Some("org.other"), now)
            .map_or(active_workspace, |intent| intent.workspace);
        assert_eq!(unrelated_window_workspace, active_workspace);
    }

    #[test]
    fn follower_expires_and_moving_its_window_retires_it() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(403);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("follower".into(), None, workspace, now);
        tracker.take_token("follower", now).unwrap();
        tracker.set_follower("follower", None, Some("org.app".into()), now);
        tracker.set_follower_window(
            "follower",
            crate::window::mapped::MappedId::next(),
            Some(workspace),
            now,
        );

        assert!(tracker.retire_follower_if_moved(
            "follower",
            WorkspaceId::specific(404),
            now + Duration::from_secs(1)
        ));
        assert!(tracker
            .follower_for_token("follower", now + Duration::from_secs(1))
            .is_none());

        tracker.prepare("expires".into(), None, workspace, now);
        tracker.take_token("expires", now).unwrap();
        tracker.set_follower("expires", None, Some("org.app".into()), now);
        assert!(tracker
            .peek_follower(&[], Some("org.app"), now + LAUNCH_WORKSPACE_TIMEOUT)
            .is_none());
    }

    #[test]
    fn ambiguous_app_ids_do_not_choose_between_live_followers() {
        let now = Instant::now();
        let mut tracker = LaunchTracker::default();
        for (token, workspace) in [
            ("first", WorkspaceId::specific(405)),
            ("second", WorkspaceId::specific(406)),
        ] {
            tracker.prepare(token.into(), None, workspace, now);
            tracker.take_token(token, now).unwrap();
            tracker.set_follower(token, None, Some("org.app".into()), now);
        }
        assert!(tracker.peek_follower(&[], Some("org.app"), now).is_none());
    }
}
