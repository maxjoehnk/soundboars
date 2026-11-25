//! Caches decoded sounds by path and decodes missing ones on a background worker thread, so the
//! caller never blocks on decoding. Sounds someone waits for ([`SoundCache::load`]) are decoded
//! before those that are only preloaded ([`SoundCache::preload`]).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
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

/// The decodes still to do, shared with the worker thread. The worker takes them from the front.
#[derive(Default)]
struct Queue {
    state: Mutex<QueueState>,
    changed: Condvar,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Job>,
    /// Set when the cache is dropped: the worker stops.
    closed: bool,
}

impl Queue {
    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for the next job; `None` once the queue is closed.
    fn next(&self) -> Option<Job> {
        let mut state = self.lock();
        loop {
            if state.closed {
                return None;
            }
            if let Some(job) = state.jobs.pop_front() {
                return Some(job);
            }
            state = self
                .changed
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }
}

pub struct SoundCache {
    sounds: HashMap<PathBuf, Samples>,
    /// Paths being decoded, with the request whose result counts.
    pending: HashMap<PathBuf, u64>,
    next_request: u64,
    queue: Arc<Queue>,
}

impl SoundCache {
    /// Starts the worker thread. It decodes with `decode` and reports every result to `on_decoded`
    /// (on the worker thread); pass those results to [`Self::finish`]. The worker stops when the
    /// cache is dropped.
    pub fn new(
        decode: impl Fn(&Path) -> Result<Samples, DecodeError> + Send + 'static,
        on_decoded: impl Fn(Decoded) + Send + 'static,
    ) -> std::io::Result<Self> {
        let queue = Arc::<Queue>::default();
        let worker_queue = Arc::clone(&queue);
        thread::Builder::new()
            .name("sound-decoder".to_owned())
            .spawn(move || {
                while let Some(Job { path, request }) = worker_queue.next() {
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
            queue,
        })
    }

    pub fn get(&self, path: &Path) -> Option<&Samples> {
        self.sounds.get(path)
    }

    /// Decodes `path` in the background, before any preloads, unless it's cached. A queued
    /// preload of it moves to the front.
    pub fn load(&mut self, path: &Path) {
        if self.sounds.contains_key(path) {
            return;
        }
        let queue = Arc::clone(&self.queue);
        let mut state = queue.lock();
        let queued = state.jobs.iter().position(|job| job.path == path);
        if let Some(job) = queued.and_then(|index| state.jobs.remove(index)) {
            state.jobs.push_front(job);
        } else if !self.pending.contains_key(path) {
            state.jobs.push_front(self.new_job(path));
            queue.changed.notify_one();
        }
    }

    /// Decodes `path` in the background after everything queued before, unless it's cached or
    /// already being decoded.
    pub fn preload(&mut self, path: &Path) {
        if self.sounds.contains_key(path) || self.pending.contains_key(path) {
            return;
        }
        let job = self.new_job(path);
        self.queue.lock().jobs.push_back(job);
        self.queue.changed.notify_one();
    }

    /// Forgets the cached sounds that `keep` doesn't contain, to free their memory. Decodes that
    /// are running or queued are left alone.
    pub fn retain(&mut self, keep: &HashSet<PathBuf>) {
        self.sounds.retain(|path, _| keep.contains(path));
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
    /// won't be cached and a queued one is dropped; [`Self::load`] decodes it afresh.
    pub fn invalidate(&mut self, path: &Path) {
        self.sounds.remove(path);
        self.pending.remove(path);
        self.queue.lock().jobs.retain(|job| job.path != path);
    }

    /// Registers a decode of `path` as pending; the caller queues the job.
    fn new_job(&mut self, path: &Path) -> Job {
        let request = self.next_request;
        self.next_request += 1;
        self.pending.insert(path.to_owned(), request);
        Job {
            path: path.to_owned(),
            request,
        }
    }
}

impl Drop for SoundCache {
    fn drop(&mut self) {
        self.queue.lock().closed = true;
        self.queue.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
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
        while !cache.queue.lock().jobs.is_empty() {
            thread::yield_now(); // until the worker took the job and waits at the gate
        }
        cache.invalidate(path);
        cache.load(path);
        drop(held);

        assert!(cache.finish(next(&rx)).is_none(), "stale result");
        assert!(cache.get(path).is_none());
        assert!(cache.finish(next(&rx)).is_some(), "fresh result");
        assert!(cache.get(path).is_some());
    }

    #[test]
    fn loads_jump_ahead_of_preloads() {
        let gate = Arc::new(Mutex::new(()));
        let (mut cache, rx) = cache_with(Arc::clone(&gate));
        let held = gate.lock().expect("gate");
        for name in ["a", "bb", "ccc"] {
            cache.preload(Path::new(name));
        }
        cache.load(Path::new("ccc"));
        drop(held);

        let order: Vec<PathBuf> = (0..3).map(|_| next(&rx).path).collect();
        let position = |name: &str| order.iter().position(|path| path == Path::new(name));
        assert!(position("ccc") < position("bb"), "{order:?}");
    }

    #[test]
    fn preloading_a_cached_or_pending_sound_does_nothing() {
        let (mut cache, rx) = cache();
        cache.load(Path::new("abc"));
        cache.preload(Path::new("abc"));
        cache.finish(next(&rx));
        cache.preload(Path::new("abc"));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }

    #[test]
    fn retain_forgets_other_sounds() {
        let (mut cache, rx) = cache();
        for name in ["a", "bb"] {
            cache.preload(Path::new(name));
            cache.finish(next(&rx));
        }
        cache.retain(&HashSet::from([PathBuf::from("a")]));
        assert!(cache.get(Path::new("a")).is_some());
        assert!(cache.get(Path::new("bb")).is_none());
    }

    #[test]
    fn invalidate_drops_a_queued_decode() {
        let gate = Arc::new(Mutex::new(()));
        let (mut cache, rx) = cache_with(Arc::clone(&gate));
        let held = gate.lock().expect("gate");
        cache.preload(Path::new("a"));
        cache.preload(Path::new("bb"));
        cache.invalidate(Path::new("bb"));
        drop(held);

        let first = next(&rx);
        assert_eq!(first.path, Path::new("a"));
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    }
}
