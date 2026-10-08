use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct RunningJob {
    id: String,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
    pause_allowed: bool,
}

/// A single active metadata/search/scan operation avoids competing with
/// itself for large volumes and prevents one cancellation from killing
/// another operation. No global reusable cancellation flag.
#[derive(Default)]
pub struct ScanJobs {
    active: Mutex<Option<RunningJob>>,
}

impl ScanJobs {
    pub fn start(&self, id: String) -> Result<Arc<AtomicBool>, String> {
        self.start_inner(id, false).map(|(cancel, _)| cancel)
    }

    /// Only the metadata indexer supports pausing. Keep the other commands'
    /// existing cancellation semantics, and isolate pause to the active job.
    pub fn start_index(&self, id: String) -> Result<(Arc<AtomicBool>, Arc<AtomicBool>), String> {
        self.start_inner(id, true)
    }

    fn start_inner(&self, id: String, pause_allowed: bool)
        -> Result<(Arc<AtomicBool>, Arc<AtomicBool>), String> {
        if id.len() < 8 || id.len() > 100 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err("Identificador de operação inválido.".into());
        }
        let mut state = self.active.lock().map_err(|_| "Estado do scanner indisponível.")?;
        if state.is_some() {
            return Err("Outra varredura ou pesquisa já está em andamento.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        *state = Some(RunningJob {
            id, cancel: Arc::clone(&cancel), pause: Arc::clone(&pause), pause_allowed,
        });
        Ok((cancel, pause))
    }

    pub fn cancel(&self, id: &str) -> Result<bool, String> {
        let state = self.active.lock().map_err(|_| "Estado do scanner indisponível.")?;
        if let Some(job) = state.as_ref().filter(|job| job.id == id) {
            job.cancel.store(true, Ordering::Relaxed);
            // Always wake a paused indexer so it can observe cancellation.
            job.pause.store(false, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn set_index_paused(&self, id: &str, paused: bool) -> Result<bool, String> {
        let state = self.active.lock().map_err(|_| "Estado do scanner indisponível.")?;
        match state.as_ref().filter(|job| job.id == id && job.pause_allowed) {
            Some(job) if !job.cancel.load(Ordering::Relaxed) => {
                job.pause.store(paused, Ordering::Relaxed);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    pub fn finish(&self, id: &str) {
        if let Ok(mut state) = self.active.lock() {
            if state.as_ref().is_some_and(|job| job.id == id) {
                *state = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cancellation_is_isolated_and_cannot_reset_a_new_session() {
        let jobs = ScanJobs::default();
        let first = jobs.start("session-one".into()).unwrap();
        assert!(jobs.start("session-two".into()).is_err());
        assert!(!jobs.cancel("session-two").unwrap());
        assert!(!first.load(Ordering::Relaxed));
        assert!(jobs.cancel("session-one").unwrap());
        assert!(first.load(Ordering::Relaxed));
        jobs.finish("session-one");
        let second = jobs.start("session-two".into()).unwrap();
        jobs.finish("session-one");
        assert!(!second.load(Ordering::Relaxed));
        assert!(jobs.cancel("session-two").unwrap());
        jobs.finish("session-two");
        assert!(!jobs.cancel("session-two").unwrap());
    }
    #[test]
    fn pause_is_per_job_and_cancel_wakes_paused_index() {
        let jobs = ScanJobs::default();
        let (cancel, pause) = jobs.start_index("index-session".into()).unwrap();
        assert!(!jobs.set_index_paused("not-index", true).unwrap());
        assert!(jobs.set_index_paused("index-session", true).unwrap());
        assert!(pause.load(Ordering::Relaxed));
        assert!(jobs.cancel("index-session").unwrap());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!pause.load(Ordering::Relaxed));
        assert!(!jobs.set_index_paused("index-session", true).unwrap());
        jobs.finish("index-session");
        let _normal = jobs.start("regular-session".into()).unwrap();
        assert!(!jobs.set_index_paused("regular-session", true).unwrap());
        jobs.finish("regular-session");
    }

    #[test]
    fn invalid_identifiers_are_rejected() {
        let jobs = ScanJobs::default();
        assert!(jobs.start("bad!".into()).is_err());
        assert!(jobs.start("short".into()).is_err());
    }
}
