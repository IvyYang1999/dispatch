//! Shared client probes: synchronous callers may wait; peer readers must not.
use std::sync::{Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant};

#[cfg(test)]
std::thread_local! { static FORBID_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }

#[cfg(test)]
pub(crate) fn without_blocking_probes<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset { fn drop(&mut self) { FORBID_SYNC.set(self.0); } }
    let _reset = Reset(FORBID_SYNC.replace(true));
    f()
}

pub(crate) struct Snapshot<T> {
    value: Mutex<Option<(Instant, T)>>,
    probe: Mutex<()>,
    scheduled: AtomicBool,
}

impl<T: Clone + Send + 'static> Snapshot<T> {
    pub const fn new() -> Self {
        Self { value: Mutex::new(None), probe: Mutex::new(()), scheduled: AtomicBool::new(false) }
    }

    fn fresh(&self, ttl: Duration) -> Option<T> {
        self.value.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            .filter(|(at, _)| at.elapsed() < ttl).map(|(_, value)| value.clone())
    }

    pub fn peek(&self) -> Option<T> {
        self.value.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|(_, v)| v.clone())
    }

    pub fn get(&self, ttl: Duration, load: impl FnOnce(Option<T>) -> T) -> T {
        #[cfg(test)]
        assert!(!FORBID_SYNC.get(), "peer response used a synchronous client probe");
        if let Some(value) = self.fresh(ttl) { return value; }
        let _probe = self.probe.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(value) = self.fresh(ttl) { return value; }
        let previous = self.peek();
        let value = load(previous);
        *self.value.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), value.clone()));
        value
    }

    pub fn read(&'static self, ttl: Duration, load: impl FnOnce(Option<T>) -> T + Send + 'static) -> Option<T> {
        if let Some(value) = self.fresh(ttl) { return Some(value); }
        let previous = self.peek();
        if self.scheduled.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            let started = std::thread::Builder::new().name("dispatch-client-probe".into()).spawn(move || {
                struct Reset<'a>(&'a AtomicBool);
                impl Drop for Reset<'_> {
                    fn drop(&mut self) { self.0.store(false, Ordering::Release); }
                }
                let _reset = Reset(&self.scheduled);
                self.get(ttl, load);
            });
            if started.is_err() { self.scheduled.store(false, Ordering::Release); }
        }
        previous
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicUsize, Ordering}, mpsc};

    #[test]
    fn a_slow_refresh_returns_the_previous_snapshot_and_runs_once() {
        let cache: &'static Snapshot<i32> = Box::leak(Box::new(Snapshot::new()));
        assert_eq!(cache.get(Duration::from_secs(60), |_| 7), 7);
        let calls = Arc::new(AtomicUsize::new(0));
        let n = calls.clone();
        let (entered_tx, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let start = Instant::now();
        let current = cache.read(Duration::ZERO, move |_| {
            n.fetch_add(1, Ordering::SeqCst);
            entered_tx.send(()).unwrap();
            let _ = wait.recv_timeout(Duration::from_millis(500));
            9
        });
        let elapsed = start.elapsed();
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        if elapsed >= Duration::from_millis(200) {
            let _ = release.send(());
            panic!("snapshot reader waited for the probe: {elapsed:?}");
        }
        assert_eq!(current, Some(7));
        for _ in 0..20 {
            let n = calls.clone();
            assert_eq!(cache.read(Duration::ZERO, move |_| { n.fetch_add(1, Ordering::SeqCst); 99 }), Some(7));
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.send(()).unwrap();
        drop(cache.probe.lock().unwrap());
        assert_eq!(cache.get(Duration::from_secs(60), |_| panic!("should reuse the refreshed snapshot")), 9);
    }

    #[test]
    fn a_cold_reader_does_not_wait_and_a_synchronous_reader_reuses_its_probe() {
        let cache: &'static Snapshot<i32> = Box::leak(Box::new(Snapshot::new()));
        let (entered_tx, entered) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        assert_eq!(cache.read(Duration::from_secs(60), move |_| {
            entered_tx.send(()).unwrap();
            wait.recv_timeout(Duration::from_secs(2)).unwrap();
            11
        }), None);
        entered.recv_timeout(Duration::from_secs(1)).unwrap();
        let caller = std::thread::spawn(move || cache.get(Duration::from_secs(60), |_| panic!("duplicate probe")));
        release.send(()).unwrap();
        assert_eq!(caller.join().unwrap(), 11);
    }
}
