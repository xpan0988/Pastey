//! The Codex threads Pastey's own app-servers have open on this Host.
//!
//! Two kinds of Pastey app-server hold a thread's writer lock:
//! - the live session of Pastey's own conversation in a folder (Run a task),
//!   kept open between tasks so the next one continues it, until idle
//!   sessions are trimmed or Pastey quits;
//! - a continuation of an existing thread (`thread_continuation`), open for
//!   exactly one turn.
//!
//! Both are known here by thread id. Session discovery uses that to say
//! "Pastey has it open" instead of blaming the Codex app or a terminal, and a
//! continuation closes Pastey's own *idle* session of the same thread first:
//! that loses nothing (the conversation stays saved, and Pastey reopens it
//! with `thread/resume` next time) and leaves Codex's own single-writer
//! refusal as the only gate. A session a task is using is never closed.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc, Mutex},
    time::{Duration, Instant},
};

use super::SessionStoreV1;

/// One thread a Pastey app-server has open.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OwnThreadHoldV1 {
    pub(crate) thread_id: String,
    /// A Pastey task is running a turn in it now; otherwise Pastey's
    /// conversation keeps it open, idle.
    pub(crate) running: bool,
}

/// What closing Pastey's own idle hold of a thread did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::native_agent) enum OwnHoldReleaseV1 {
    /// No Pastey session had the thread open.
    NotHeld,
    /// Pastey's idle session of the thread was closed.
    Released,
    /// A Pastey task is using it now; nothing was closed.
    InUse,
}

#[derive(Clone, Default)]
pub(in crate::native_agent) struct OwnThreadHoldsV1 {
    sessions: SessionStoreV1,
    continuing: Arc<Mutex<HashSet<String>>>,
}

impl OwnThreadHoldsV1 {
    pub(super) fn new(sessions: SessionStoreV1) -> Self {
        Self {
            sessions,
            continuing: Arc::default(),
        }
    }

    pub(in crate::native_agent) fn observe(&self) -> Vec<OwnThreadHoldV1> {
        let mut holds: Vec<OwnThreadHoldV1> = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .values()
                    .map(|session| OwnThreadHoldV1 {
                        thread_id: session.thread_id.clone(),
                        running: session.controller.in_use.load(Ordering::Acquire),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Ok(continuing) = self.continuing.lock() {
            for thread_id in continuing.iter() {
                holds.retain(|hold| &hold.thread_id != thread_id);
                holds.push(OwnThreadHoldV1 {
                    thread_id: thread_id.clone(),
                    running: true,
                });
            }
        }
        holds
    }

    /// Closes Pastey's own idle session of `thread_id`, if there is one.
    pub(in crate::native_agent) fn release_idle(&self, thread_id: &str) -> OwnHoldReleaseV1 {
        let Ok(mut sessions) = self.sessions.lock() else {
            return OwnHoldReleaseV1::NotHeld;
        };
        let held: Vec<String> = sessions
            .iter()
            .filter(|(_, session)| session.thread_id == thread_id)
            .map(|(key, _)| key.clone())
            .collect();
        if held.is_empty() {
            return OwnHoldReleaseV1::NotHeld;
        }
        if held.iter().any(|key| {
            sessions
                .get(key)
                .is_some_and(|session| session.controller.in_use.load(Ordering::Acquire))
        }) {
            return OwnHoldReleaseV1::InUse;
        }
        let closed: Vec<_> = held.iter().filter_map(|key| sessions.remove(key)).collect();
        drop(sessions);
        for session in closed {
            session.controller.shutdown();
        }
        OwnHoldReleaseV1::Released
    }

    /// Marks `thread_id` as continued by Pastey until the guard drops.
    pub(in crate::native_agent) fn continuing(&self, thread_id: &str) -> OwnContinuationGuardV1 {
        if let Ok(mut continuing) = self.continuing.lock() {
            continuing.insert(thread_id.to_owned());
        }
        OwnContinuationGuardV1 {
            continuing: self.continuing.clone(),
            thread_id: thread_id.to_owned(),
        }
    }
}

pub(in crate::native_agent) struct OwnContinuationGuardV1 {
    continuing: Arc<Mutex<HashSet<String>>>,
    thread_id: String,
}

impl Drop for OwnContinuationGuardV1 {
    fn drop(&mut self) {
        if let Ok(mut continuing) = self.continuing.lock() {
            continuing.remove(&self.thread_id);
        }
    }
}

/// After Pastey closed its own session of a thread, Codex removes the lock
/// file when that app-server has exited. Waits a bounded time for that, so
/// the continuation's `thread/resume` does not race the exit (behind a
/// Windows launcher the app-server can outlive its launcher briefly). A file
/// left behind is no reason to refuse: Codex's own refusal still decides.
pub(in crate::native_agent) fn wait_for_lock_release(codex_home: Option<&Path>, thread_id: &str) {
    let Some(lock) = codex_home.map(|home| lock_path(home, thread_id)) else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn lock_path(codex_home: &Path, thread_id: &str) -> PathBuf {
    codex_home
        .join("thread-writer-locks")
        .join(format!("{thread_id}.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continuations_are_reported_running_until_their_guard_drops() {
        let holds = OwnThreadHoldsV1::default();
        assert!(holds.observe().is_empty());
        assert_eq!(holds.release_idle("t-1"), OwnHoldReleaseV1::NotHeld);
        let guard = holds.continuing("t-1");
        assert_eq!(
            holds.observe(),
            vec![OwnThreadHoldV1 {
                thread_id: "t-1".into(),
                running: true
            }]
        );
        drop(guard);
        assert!(holds.observe().is_empty());
    }

    /// Pastey's own idle session of a thread is closed for a continuation;
    /// one a task is using is left alone, and others are untouched.
    #[cfg(unix)]
    #[test]
    fn only_pastey_s_own_idle_session_of_that_thread_is_closed() {
        use super::super::{CodexAppServerV1, CodexLauncherV1, NativeCodexSessionV1};
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("pastey-own-holds-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("received.log");
        let program = root.join("codex-fixture");
        std::fs::write(
            &program,
            format!(
                "#!/bin/sh\necho $$ >> '{}'\nwhile IFS= read -r line; do\n  case \"$line\" in *'\"id\":1,'*) echo '{{\"id\":1,\"result\":{{}}}}' ;; esac\ndone\n",
                log.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
        let launcher = CodexLauncherV1 {
            program,
            search_path: None,
            backend: "configured",
            version: None,
        };
        let sessions = SessionStoreV1::default();
        for (key, thread) in [("mine", "t-1"), ("other", "t-2")] {
            let controller = Arc::new(CodexAppServerV1::launch(&launcher, &root).unwrap());
            sessions.lock().unwrap().insert(
                key.into(),
                NativeCodexSessionV1 {
                    workspace: root.clone(),
                    controller,
                    thread_id: thread.into(),
                    last_used: Instant::now(),
                },
            );
        }
        let holds = OwnThreadHoldsV1::new(sessions.clone());
        assert_eq!(holds.observe().len(), 2);
        assert!(holds.observe().iter().all(|hold| !hold.running));

        let in_use = sessions.lock().unwrap()["mine"].controller.clone();
        in_use.in_use.store(true, Ordering::Release);
        assert!(holds
            .observe()
            .iter()
            .any(|hold| hold.thread_id == "t-1" && hold.running));
        assert_eq!(holds.release_idle("t-1"), OwnHoldReleaseV1::InUse);
        assert_eq!(sessions.lock().unwrap().len(), 2, "a session in use stays");

        in_use.in_use.store(false, Ordering::Release);
        assert_eq!(holds.release_idle("t-1"), OwnHoldReleaseV1::Released);
        let left: Vec<String> = sessions
            .lock()
            .unwrap()
            .values()
            .map(|s| s.thread_id.clone())
            .collect();
        assert_eq!(left, ["t-2"]);
        // Launched in order: the first app-server (t-1) is gone, the other runs.
        let pids: Vec<String> = std::fs::read_to_string(&log)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        let alive = |pid: &str| {
            std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap()
                .success()
        };
        assert_eq!((alive(&pids[0]), alive(&pids[1])), (false, true));
        assert_eq!(holds.release_idle("t-1"), OwnHoldReleaseV1::NotHeld);
        for session in sessions.lock().unwrap().values() {
            session.controller.shutdown();
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
