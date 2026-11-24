//! Finds out how long the library's sounds are, on a background thread, so tiles can show it
//! before a sound was ever played.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use crate::audio::decode::DecodeError;

/// A sound's length, or why it couldn't be determined.
pub type Probed = (PathBuf, Result<Duration, DecodeError>);

pub struct DurationProbe {
    jobs: mpsc::Sender<PathBuf>,
    results: mpsc::Receiver<Probed>,
    /// Paths asked for, so each is probed once until [`Self::forget`].
    requested: HashSet<PathBuf>,
}

impl DurationProbe {
    /// Starts the worker thread; it measures sounds with `probe` (e.g.
    /// [`crate::audio::decode::probe_duration`]) and stops when the probe is dropped.
    pub fn start(
        probe: impl Fn(&Path) -> Result<Duration, DecodeError> + Send + 'static,
    ) -> std::io::Result<Self> {
        let (jobs, job_rx) = mpsc::channel::<PathBuf>();
        let (result_tx, results) = mpsc::channel();
        thread::Builder::new()
            .name("duration-probe".to_owned())
            .spawn(move || {
                for path in job_rx {
                    let duration = probe(&path);
                    if result_tx.send((path, duration)).is_err() {
                        return;
                    }
                }
            })?;
        Ok(Self {
            jobs,
            results,
            requested: HashSet::new(),
        })
    }

    /// Measures the sound at `path` in the background, unless that was asked for before.
    pub fn request(&mut self, path: &Path) {
        if self.requested.insert(path.to_owned()) {
            // Fails only if the worker panicked; then the length just stays unknown.
            let _ = self.jobs.send(path.to_owned());
        }
    }

    /// Forgets that `path` was measured, e.g. because the file changed; request it again.
    pub fn forget(&mut self, path: &Path) {
        self.requested.remove(path);
    }

    /// The results since the last call.
    pub fn poll(&self) -> Vec<Probed> {
        self.results.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    /// A probe whose "sounds" are as many milliseconds long as their name has characters.
    fn probe() -> DurationProbe {
        DurationProbe::start(|path: &Path| {
            if path.starts_with("bad") {
                return Err(DecodeError::NoAudio);
            }
            Ok(Duration::from_millis(path.as_os_str().len() as u64))
        })
        .expect("spawns")
    }

    fn wait_for(probe: &DurationProbe, count: usize) -> Vec<Probed> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut results = Vec::new();
        while results.len() < count {
            assert!(Instant::now() < deadline, "only {results:?}");
            results.extend(probe.poll());
            thread::sleep(Duration::from_millis(5));
        }
        thread::sleep(Duration::from_millis(50));
        results.extend(probe.poll());
        results
    }

    #[test]
    fn measures_each_sound_once_until_forgotten() {
        let mut probe = probe();
        probe.request(Path::new("abc"));
        probe.request(Path::new("abc"));
        probe.request(Path::new("bad"));
        let results = wait_for(&probe, 2);
        assert_eq!(results.len(), 2, "{results:?}");
        assert_eq!(results[0].0, Path::new("abc"));
        assert_eq!(results[0].1.as_ref().ok(), Some(&Duration::from_millis(3)));
        assert!(results[1].1.is_err());

        probe.forget(Path::new("abc"));
        probe.request(Path::new("abc"));
        assert_eq!(wait_for(&probe, 1).len(), 1);
    }
}
