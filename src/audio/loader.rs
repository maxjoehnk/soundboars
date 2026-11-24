//! Caches decoded sounds by path and decodes missing ones on a background worker thread, so the
//! caller never blocks on decoding.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use super::decode::{DecodeError, Samples};

/// The outcome of a background decode; hand it back to [`SoundCache::finish`].
#[derive(Debug)]
pub struct Decoded {
    pub path: PathBuf,
    pub result: Result<Samples, DecodeError>,
    /// Tells a current decode from one that was invalidated while it ran.
    request: u64,
}

struct Job {
    path: PathBuf,
    request: u64,
}

pub struct SoundCache {
    sounds: HashMap<PathBuf, Samples>,
    /// Paths being decoded, with the request whose result counts.
    pending: HashMap<PathBuf, u64>,
    next_request: u64,
    jobs: mpsc::Sender<Job>,
}

impl SoundCache {
    /// Starts the worker thread. It decodes with `decode` and reports every result to `on_decoded`
    /// (on the worker thread); pass those results to [`Self::finish`]. The worker stops when the
    /// cache is dropped.
    pub fn new(
        decode: impl Fn(&Path) -> Result<Samples, DecodeError> + Send + 'static,
        on_decoded: impl Fn(Decoded) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (jobs, job_rx) = mpsc::channel::<Job>();
        thread::Builder::new()
            .name("sound-decoder".to_owned())
            .spawn(move || {
                for Job { path, request } in job_rx {
                    let result = decode(&path);
                    on_decoded(Decoded {
                        path,
                        result,
                        request,
                    });
                }
            })?;
        Ok(Self {
            sounds: HashMap::new(),
            pending: HashMap::new(),
            next_request: 0,
            jobs,
        })
    }

    pub fn get(&self, path: &Path) -> Option<&Samples> {
        self.sounds.get(path)
    }

    /// Starts decoding `path` in the background unless it's cached or already being decoded.
    pub fn load(&mut self, path: &Path) {
        if self.sounds.contains_key(path) || self.pending.contains_key(path) {
            return;
        }
        let request = self.next_request;
        self.next_request += 1;
        self.pending.insert(path.to_owned(), request);
        // Fails only if the worker panicked; then this path just never finishes loading.
        let _ = self.jobs.send(Job {
            path: path.to_owned(),
            request,
        });
    }

    /// Takes in a background result: caches the sound and returns the result, or returns `None` if
    /// the decode was invalidated (or superseded) while it ran.
    pub fn finish(&mut self, decoded: Decoded) -> Option<Result<Samples, DecodeError>> {
        if self.pending.get(&decoded.path) != Some(&decoded.request) {
            return None;
        }
        self.pending.remove(&decoded.path);
        if let Ok(samples) = &decoded.result {
            self.sounds.insert(decoded.path, Samples::clone(samples));
        }
        Some(decoded.result)
    }

    /// Forgets `path`, e.g. because the file changed on disk. A decode of it that is still running
    /// won't be cached; [`Self::load`] decodes it afresh.
    pub fn invalidate(&mut self, path: &Path) {
        self.sounds.remove(path);
        self.pending.remove(path);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// A cache whose "decoder" returns one frame holding the file name's length, or fails for
    /// names starting with "bad". Results arrive on the returned channel. The decoder waits on
    /// `gate` first, so tests can hold decodes back.
    fn cache_with(gate: Arc<Mutex<()>>) -> (SoundCache, mpsc::Receiver<Decoded>) {
        let (tx, rx) = mpsc::channel();
        let decode = move |path: &Path| {
            let _open = gate.lock().expect("gate");
            let name = path.to_string_lossy();
            if name.starts_with("bad") {
                return Err(DecodeError::NoAudio);
            }
            Ok(Samples::from(vec![name.len() as f32; 2]))
        };
        let on_decoded = move |decoded| {
            let _ = tx.send(decoded);
        };
        (SoundCache::new(decode, on_decoded).expect("spawns"), rx)
    }

    fn cache() -> (SoundCache, mpsc::Receiver<Decoded>) {
        cache_with(Arc::default())
    }

    fn next(rx: &mpsc::Receiver<Decoded>) -> Decoded {
        rx.recv_timeout(TIMEOUT).expect("a decode finishes")
    }

    #[test]
    fn decodes_in_the_background_and_caches() {
        let (mut cache, rx) = cache();
        let path = Path::new("abc");
        assert!(cache.get(path).is_none());

        cache.load(path);
        let result = cache.finish(next(&rx)).expect("current");
        assert_eq!(&*result.expect("decodes"), [3.0, 3.0]);
        assert_eq!(cache.get(path).map(|s| s.len()), Some(2));

        cache.load(path);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn loading_twice_decodes_once() {
        let (mut cache, rx) = cache();
        cache.load(Path::new("abc"));
        cache.load(Path::new("abc"));
        cache.finish(next(&rx));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn errors_are_reported_and_not_cached() {
        let (mut cache, rx) = cache();
        let path = Path::new("bad.mp3");
        cache.load(path);
        let result = cache.finish(next(&rx)).expect("current");
        assert!(matches!(result, Err(DecodeError::NoAudio)));
        assert!(cache.get(path).is_none());

        cache.load(path);
        assert!(cache.finish(next(&rx)).is_some(), "retried");
    }

    #[test]
    fn invalidate_forgets_a_cached_sound() {
        let (mut cache, rx) = cache();
        let path = Path::new("abc");
        cache.load(path);
        cache.finish(next(&rx));

        cache.invalidate(path);
        assert!(cache.get(path).is_none());
        cache.load(path);
        assert!(cache.finish(next(&rx)).is_some());
    }

    #[test]
    fn a_decode_invalidated_while_running_is_discarded() {
        let gate = Arc::new(Mutex::new(()));
        let (mut cache, rx) = cache_with(Arc::clone(&gate));
        let path = Path::new("abc");
        let held = gate.lock().expect("gate");
        cache.load(path);
        cache.invalidate(path);
        cache.load(path);
        drop(held);

        assert!(cache.finish(next(&rx)).is_none(), "stale result");
        assert!(cache.get(path).is_none());
        assert!(cache.finish(next(&rx)).is_some(), "fresh result");
        assert!(cache.get(path).is_some());
    }
}
