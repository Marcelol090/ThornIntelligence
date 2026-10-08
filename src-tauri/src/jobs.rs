use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

struct RunningJob {
    id: String,
    cancel: Arc<AtomicBool>,
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
        if id.len() < 8 || id.len() > 100 || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err("Identificador de operação inválido.".into());
        }
        let mut state = self.active.lock().map_err(|_| "Estado do scanner indisponível.")?;
        if state.is_some() {
            return Err("Outra varredura ou pesquisa já está em andamento.".into());
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *state = Some(RunningJob { id, cancel: Arc::clone(&cancel) });
        Ok(cancel)
    }

    pub fn cancel(&self, id: &str) -> Result<bool, String> {
        let state = self.active.lock().map_err(|_| "Estado do scanner indisponível.")?;
        if let Some(job) = state.as_ref().filter(|job| job.id == id) {
            job.cancel.store(true, Ordering::Relaxed);
            Ok(true)
        } else {
            Ok(false)
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
    fn invalid_identifiers_are_rejected() {
        let jobs = ScanJobs::default();
        assert!(jobs.start("bad!".into()).is_err());
        assert!(jobs.start("short".into()).is_err());
    }
}
