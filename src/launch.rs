//! Bounded launch-to-window workspace affinity.

use std::time::{Duration, Instant};

use crate::launch_process::ProcessIdentity;
use crate::layout::workspace::WorkspaceId;

/// Launch intents expire at this boundary.
pub const LAUNCH_WORKSPACE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_PENDING_LAUNCHES: usize = 128;

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
}

#[derive(Debug, Default)]
pub struct LaunchTracker {
    pending: Vec<LaunchIntent>,
}

impl LaunchTracker {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
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
}
