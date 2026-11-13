//! Bounded launch-to-window workspace affinity.

use std::time::{Duration, Instant};

use crate::launch_process::ProcessIdentity;
use crate::layout::workspace::WorkspaceId;

/// Launch records expire at this boundary.
pub const LAUNCH_WORKSPACE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PENDING_LAUNCHES: usize = 128;

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

/// The innermost systemd unit named in a `/proc/<pid>/cgroup` listing.
pub fn unit_from_cgroup(cgroup: &[u8]) -> Option<&str> {
    std::str::from_utf8(cgroup)
        .ok()?
        .lines()
        .filter_map(|line| line.rsplit('/').next())
        .find(|name| {
            name.len() <= 255 && (name.ends_with(".service") || name.ends_with(".scope"))
        })
}

/// One launch. Until its first window maps it is a pending intent; afterwards it stays for its
/// 60 s lifetime as a follower so later toplevels of the same app land on the launch workspace.
#[derive(Debug)]
pub struct LaunchIntent {
    pub token: String,
    pub unit: Option<String>,
    /// The launched process (exec gate) or the first matched client; descendants follow.
    pub process: Option<ProcessIdentity>,
    /// App ID of the first mapped window; a unique match follows.
    pub app_id: Option<String>,
    pub workspace: WorkspaceId,
    pub created: Instant,
    pub consumed: bool,
    /// Where the first window landed; a window of this launch seen elsewhere retires it.
    pub placed: Option<WorkspaceId>,
}

#[derive(Debug, Default)]
pub struct LaunchTracker {
    pending: Vec<LaunchIntent>,
}

impl LaunchTracker {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn prepare(
        &mut self,
        token: String,
        unit: Option<String>,
        workspace: WorkspaceId,
        now: Instant,
    ) {
        if let Some(existing) = self.get(&token, now) {
            if !existing.consumed && unit.is_some() {
                existing.unit = unit;
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
            app_id: None,
            workspace,
            created: now,
            consumed: false,
            placed: None,
        });
    }

    pub fn get(&mut self, token: &str, now: Instant) -> Option<&mut LaunchIntent> {
        self.expire(now);
        self.pending.iter_mut().find(|intent| intent.token == token)
    }

    /// The not yet consumed record for `token`.
    pub fn pending(&mut self, token: &str, now: Instant) -> Option<&mut LaunchIntent> {
        self.get(token, now).filter(|intent| !intent.consumed)
    }

    pub fn find_unit(&mut self, unit: &str, now: Instant) -> Option<&mut LaunchIntent> {
        self.expire(now);
        self.pending
            .iter_mut()
            .find(|intent| !intent.consumed && intent.unit.as_deref() == Some(unit))
    }

    /// Match a toplevel by process ancestry, else by an app ID that names exactly one launch.
    pub fn find_process(
        &mut self,
        ancestry: &[ProcessIdentity],
        app_id: Option<&str>,
        now: Instant,
    ) -> Option<&mut LaunchIntent> {
        self.expire(now);
        let by_process = self
            .pending
            .iter()
            .position(|intent| intent.process.is_some_and(|root| ancestry.contains(&root)));
        let index = by_process.or_else(|| {
            let app_id = app_id.filter(|app_id| !app_id.is_empty())?;
            let mut matches = self
                .pending
                .iter()
                .enumerate()
                .filter(|(_, intent)| intent.app_id.as_deref() == Some(app_id))
                .map(|(index, _)| index);
            let index = matches.next()?;
            matches.next().is_none().then_some(index)
        })?;
        self.pending.get_mut(index)
    }

    pub fn discard(&mut self, token: &str, now: Instant) {
        self.expire(now);
        self.pending.retain(|intent| intent.token != token);
    }

    /// Record where each launch's first window landed, and retire a launch once one of its
    /// windows is found elsewhere, so a relocated app cannot drag later windows back.
    pub fn retire_moved<'a>(
        &mut self,
        windows: impl Iterator<Item = (&'a str, WorkspaceId)>,
        now: Instant,
    ) {
        self.expire(now);
        for (token, workspace) in windows {
            let Some(intent) = self.pending.iter_mut().find(|intent| intent.token == token) else {
                continue;
            };
            if intent.placed.is_none() {
                intent.placed = Some(workspace);
            } else if intent.placed != Some(workspace) {
                self.pending.retain(|intent| intent.token != token);
            }
        }
    }

    fn expire(&mut self, now: Instant) {
        self.pending
            .retain(|intent| now.duration_since(intent.created) < LAUNCH_WORKSPACE_TIMEOUT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn consume(tracker: &mut LaunchTracker, token: &str, app_id: &str, now: Instant) {
        let intent = tracker.pending(token, now).unwrap();
        intent.consumed = true;
        intent.app_id = Some(app_id.into());
    }

    #[test]
    fn expires_at_sixty_seconds_including_the_exact_boundary() {
        let now = Instant::now();
        let mut tracker = LaunchTracker::default();
        tracker.prepare("t".into(), None, WorkspaceId::specific(101), now);
        assert!(tracker
            .pending("t", now + LAUNCH_WORKSPACE_TIMEOUT - Duration::from_nanos(1))
            .is_some());
        assert!(tracker.get("t", now + LAUNCH_WORKSPACE_TIMEOUT).is_none());
    }

    #[test]
    fn binding_unit_keeps_original_workspace_and_clock_and_consumption_is_single_use() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(102);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("t".into(), None, workspace, now);
        // A re-prepared token keeps its record and may gain a unit.
        tracker.prepare(
            "t".into(),
            Some("nacre-app-a.service".into()),
            WorkspaceId::specific(1),
            now + Duration::from_secs(20),
        );
        let intent = tracker
            .find_unit("nacre-app-a.service", now + Duration::from_secs(59))
            .unwrap();
        assert_eq!(intent.workspace, workspace);
        assert_eq!(intent.created, now);
        intent.consumed = true;
        assert!(tracker.pending("t", now).is_none());
        assert!(tracker.find_unit("nacre-app-a.service", now).is_none());
        assert!(tracker.find_unit("nacre-app-other.service", now).is_none());
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
        assert!(tracker.get("token-0", now).is_none());
        assert_eq!(
            tracker.get("token-127", now).unwrap().workspace,
            WorkspaceId::specific(127)
        );
        assert_eq!(
            tracker.get("last", now).unwrap().workspace,
            WorkspaceId::specific(999)
        );
    }

    #[test]
    fn extracts_only_activation_tokens_and_units_without_exposing_other_environment() {
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
        assert_eq!(
            unit_from_cgroup(b"0::/user.slice/user-1000.slice/app-x-42.scope\n"),
            Some("app-x-42.scope")
        );
        assert_eq!(unit_from_cgroup(b"0::/user.slice/session.slice\n"), None);
    }

    #[test]
    fn process_match_requires_pid_and_start_time_and_wins_over_app_id() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(301);
        let root = ProcessIdentity {
            pid: 42,
            starttime: 100,
        };
        let mut tracker = LaunchTracker::default();
        tracker.prepare("process".into(), None, workspace, now);
        tracker.pending("process", now).unwrap().process = Some(root);
        assert!(tracker
            .find_process(
                &[ProcessIdentity {
                    pid: 42,
                    starttime: 101
                }],
                None,
                now
            )
            .is_none());
        assert_eq!(
            tracker.find_process(&[root], None, now).unwrap().token,
            "process"
        );

        // Splash then main: the consumed record still matches PID ancestry under another app ID.
        consume(&mut tracker, "process", "org.discord", now);
        assert_eq!(
            tracker
                .find_process(&[root], Some("org.other"), now)
                .unwrap()
                .workspace,
            workspace
        );
    }

    #[test]
    fn app_id_follower_matches_only_the_same_app_and_only_when_unambiguous() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(402);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("follower".into(), None, workspace, now);
        consume(&mut tracker, "follower", "org.discord", now);
        assert_eq!(
            tracker
                .find_process(&[], Some("org.discord"), now)
                .unwrap()
                .workspace,
            workspace
        );
        assert!(tracker.find_process(&[], Some("org.other"), now).is_none());
        assert!(tracker.find_process(&[], Some(""), now).is_none());

        tracker.prepare("second".into(), None, WorkspaceId::specific(406), now);
        consume(&mut tracker, "second", "org.discord", now);
        assert!(tracker.find_process(&[], Some("org.discord"), now).is_none());
    }

    #[test]
    fn follower_expires_and_moving_its_window_retires_it() {
        let now = Instant::now();
        let workspace = WorkspaceId::specific(403);
        let mut tracker = LaunchTracker::default();
        tracker.prepare("follower".into(), None, workspace, now);
        consume(&mut tracker, "follower", "org.app", now);
        tracker.retire_moved([("follower", workspace)].into_iter(), now);
        tracker.retire_moved([("follower", workspace)].into_iter(), now);
        assert!(tracker.get("follower", now).is_some());
        tracker.retire_moved(
            [("follower", WorkspaceId::specific(404))].into_iter(),
            now + Duration::from_secs(1),
        );
        assert!(tracker.get("follower", now).is_none());

        tracker.prepare("expires".into(), None, workspace, now);
        consume(&mut tracker, "expires", "org.app", now);
        assert!(tracker
            .find_process(&[], Some("org.app"), now + LAUNCH_WORKSPACE_TIMEOUT)
            .is_none());
    }
}
