//! HTTP with a global bandwidth cap, plus a prioritised worker pool.
//!
//! Every byte the game downloads (tiles, geocoding, updates) passes through one
//! token-bucket [`Limiter`], so the whole game stays under the user's cap.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering as AO};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum NetError {
    /// The server says the resource does not exist (safe to remember).
    NotFound,
    Http(u16),
    Io(String),
}

impl std::fmt::Display for NetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "not found"),
            Self::Http(c) => write!(f, "HTTP {c}"),
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

/// Token bucket shared by all downloads. Rate 0 means unlimited.
pub struct Limiter {
    bytes_per_sec: AtomicU64,
    bucket: Mutex<(Instant, f64)>,
}

impl Limiter {
    fn new(bps: u64) -> Self {
        Self { bytes_per_sec: AtomicU64::new(bps), bucket: Mutex::new((Instant::now(), 0.0)) }
    }

    pub fn set_mbps(&self, mbps: f64) {
        self.bytes_per_sec.store((mbps.max(0.0) * 1_000_000.0 / 8.0) as u64, AO::Relaxed);
    }

    pub fn mbps(&self) -> f64 {
        self.bytes_per_sec.load(AO::Relaxed) as f64 * 8.0 / 1_000_000.0
    }

    /// Block until `n` bytes may be consumed.
    pub fn take(&self, n: usize) {
        let rate = self.bytes_per_sec.load(AO::Relaxed) as f64;
        if rate <= 0.0 {
            return;
        }
        let wait = {
            let mut b = self.bucket.lock().unwrap();
            let now = Instant::now();
            // Allow a burst of a quarter second of traffic.
            b.1 = (b.1 + now.duration_since(b.0).as_secs_f64() * rate).min(rate * 0.25);
            b.0 = now;
            b.1 -= n as f64;
            if b.1 < 0.0 { -b.1 / rate } else { 0.0 }
        };
        if wait > 0.0 {
            std::thread::sleep(Duration::from_secs_f64(wait));
        }
    }
}

/// Running totals for the debug overlay.
#[derive(Default)]
pub struct NetStats {
    pub bytes: AtomicU64,
    pub requests: AtomicU64,
    pub errors: AtomicU64,
    pub in_flight: AtomicU64,
}

pub fn limiter() -> &'static Limiter {
    static L: OnceLock<Limiter> = OnceLock::new();
    L.get_or_init(|| Limiter::new(500_000_000 / 8))
}

pub fn stats() -> &'static NetStats {
    static S: OnceLock<NetStats> = OnceLock::new();
    S.get_or_init(NetStats::default)
}

fn agent() -> &'static ureq::Agent {
    static A: OnceLock<ureq::Agent> = OnceLock::new();
    A.get_or_init(|| {
        ureq::Agent::config_builder()
            .user_agent(crate::user_agent())
            // The system's certificate store, so proxies with their own CA work too.
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(60)))
            .http_status_as_error(false)
            .max_idle_connections_per_host(16)
            // Redirects are followed by hand (see get_with) without reading their bodies.
            .max_redirects(0)
            .build()
            .into()
    })
}

/// Download a URL fully, honouring the bandwidth cap. `progress` gets bytes so far.
pub fn get_with(url: &str, accept: &str, mut progress: impl FnMut(u64, Option<u64>)) -> Result<Vec<u8>, NetError> {
    let st = stats();
    st.requests.fetch_add(1, AO::Relaxed);
    st.in_flight.fetch_add(1, AO::Relaxed);
    let r = (|| {
        let mut url = url.to_string();
        let mut hops = 0;
        let resp = loop {
            let resp = agent().get(&url).header("Accept", accept).call().map_err(|e| NetError::Io(e.to_string()))?;
            let code = resp.status().as_u16();
            if (300..400).contains(&code) && code != 304 {
                let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).ok_or(NetError::Http(code))?.to_string();
                drop(resp);
                hops += 1;
                if hops > 6 {
                    return Err(NetError::Http(code));
                }
                url = if loc.starts_with("http") { loc } else { format!("{}{}", origin_of(&url), loc) };
                continue;
            }
            break resp;
        };
        let code = resp.status().as_u16();
        if code == 404 || code == 410 {
            return Err(NetError::NotFound);
        }
        if code >= 400 {
            return Err(NetError::Http(code));
        }
        let len = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        let mut body = resp.into_body();
        let mut reader = body.as_reader();
        let mut out = Vec::with_capacity(len.unwrap_or(32 * 1024) as usize);
        let mut buf = [0u8; 16 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(|e| NetError::Io(e.to_string()))?;
            if n == 0 {
                break;
            }
            limiter().take(n);
            st.bytes.fetch_add(n as u64, AO::Relaxed);
            out.extend_from_slice(&buf[..n]);
            progress(out.len() as u64, len);
        }
        Ok(out)
    })();
    st.in_flight.fetch_sub(1, AO::Relaxed);
    if r.is_err() && r != Err(NetError::NotFound) {
        st.errors.fetch_add(1, AO::Relaxed);
    }
    r
}

/// `https://host` part of a URL, for relative redirects.
fn origin_of(url: &str) -> &str {
    let after = url.find("://").map(|i| i + 3).unwrap_or(0);
    let end = url[after..].find('/').map(|i| i + after).unwrap_or(url.len());
    &url[..end]
}

pub fn get(url: &str) -> Result<Vec<u8>, NetError> {
    get_with(url, "*/*", |_, _| {})
}

pub fn get_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, NetError> {
    let b = get_with(url, "application/json", |_, _| {})?;
    serde_json::from_slice(&b).map_err(|e| NetError::Io(e.to_string()))
}

// ---------------------------------------------------------------- worker pool

struct Entry {
    priority: f32,
    seq: u64,
    key: u64,
}
impl PartialEq for Entry {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Entry {
    // BinaryHeap is a max-heap; lower priority value = more urgent.
    fn cmp(&self, o: &Self) -> Ordering {
        o.priority.total_cmp(&self.priority).then(o.seq.cmp(&self.seq))
    }
}

struct Queue<J> {
    heap: BinaryHeap<Entry>,
    jobs: HashMap<u64, (f32, u64, J)>,
    seq: u64,
    closed: bool,
}

/// A pool of threads working through jobs in priority order. Jobs are keyed so
/// the caller can re-prioritise or cancel them while they wait.
pub struct WorkPool<J: Send + 'static, R: Send + 'static> {
    q: Arc<(Mutex<Queue<J>>, Condvar)>,
    results: crossbeam_channel::Receiver<R>,
    busy: Arc<AtomicU64>,
}

impl<J: Send + 'static, R: Send + 'static> WorkPool<J, R> {
    pub fn new(name: &str, threads: usize, handler: impl Fn(u64, J) -> R + Send + Sync + 'static) -> Self {
        let q = Arc::new((Mutex::new(Queue { heap: BinaryHeap::new(), jobs: HashMap::new(), seq: 0, closed: false }), Condvar::new()));
        let (tx, rx) = crossbeam_channel::unbounded();
        let handler = Arc::new(handler);
        let busy = Arc::new(AtomicU64::new(0));
        for i in 0..threads.max(1) {
            let (q, tx, h, busy) = (q.clone(), tx.clone(), handler.clone(), busy.clone());
            std::thread::Builder::new()
                .name(format!("{name}-{i}"))
                .spawn(move || {
                    loop {
                        let (key, job) = {
                            let (m, cv) = &*q;
                            let mut g = m.lock().unwrap();
                            loop {
                                if g.closed {
                                    return;
                                }
                                if let Some(e) = g.heap.pop() {
                                    // Skip stale heap entries left behind by re-prioritising.
                                    if g.jobs.get(&e.key).is_some_and(|j| j.1 == e.seq) {
                                        let (_, _, job) = g.jobs.remove(&e.key).unwrap();
                                        break (e.key, job);
                                    }
                                    continue;
                                }
                                g = cv.wait(g).unwrap();
                            }
                        };
                        busy.fetch_add(1, AO::Relaxed);
                        let r = h(key, job);
                        busy.fetch_sub(1, AO::Relaxed);
                        if tx.send(r).is_err() {
                            return;
                        }
                    }
                })
                .expect("spawn worker");
        }
        Self { q, results: rx, busy }
    }

    /// Queue a job, or update the priority of one already waiting under `key`.
    pub fn submit(&self, key: u64, priority: f32, job: J) {
        let (m, cv) = &*self.q;
        let mut g = m.lock().unwrap();
        g.seq += 1;
        let seq = g.seq;
        g.jobs.insert(key, (priority, seq, job));
        g.heap.push(Entry { priority, seq, key });
        cv.notify_one();
    }

    /// Change the priority of a waiting job. Returns false if it is not waiting.
    pub fn reprioritize(&self, key: u64, priority: f32) -> bool {
        let (m, _) = &*self.q;
        let mut g = m.lock().unwrap();
        g.seq += 1;
        let seq = g.seq;
        match g.jobs.get_mut(&key) {
            Some(j) if (j.0 - priority).abs() > f32::EPSILON => {
                j.0 = priority;
                j.1 = seq;
                g.heap.push(Entry { priority, seq, key });
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Drop a waiting job. Returns true if it was still waiting.
    pub fn cancel(&self, key: u64) -> bool {
        self.q.0.lock().unwrap().jobs.remove(&key).is_some()
    }

    pub fn waiting(&self) -> usize {
        self.q.0.lock().unwrap().jobs.len()
    }

    pub fn busy(&self) -> usize {
        self.busy.load(AO::Relaxed) as usize
    }

    /// Finished results not yet collected with [`try_recv`](Self::try_recv).
    pub fn finished(&self) -> usize {
        self.results.len()
    }

    pub fn try_recv(&self) -> Option<R> {
        self.results.try_recv().ok()
    }
}

impl<J: Send + 'static, R: Send + 'static> Drop for WorkPool<J, R> {
    fn drop(&mut self) {
        self.q.0.lock().unwrap().closed = true;
        self.q.1.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_runs_in_priority_order() {
        let gate = Arc::new(Mutex::new(()));
        let g = gate.lock().unwrap();
        let gate2 = gate.clone();
        let pool = WorkPool::new("t", 1, move |k, _: ()| {
            drop(gate2.lock().unwrap());
            k
        });
        pool.submit(100, 0.0, ()); // occupies the single worker until the gate opens
        std::thread::sleep(Duration::from_millis(50));
        pool.submit(1, 5.0, ());
        pool.submit(2, 1.0, ());
        pool.submit(3, 3.0, ());
        pool.reprioritize(1, 0.5);
        pool.cancel(3);
        drop(g);
        let mut got = Vec::new();
        let t = Instant::now();
        while got.len() < 3 && t.elapsed() < Duration::from_secs(5) {
            if let Some(k) = pool.try_recv() {
                got.push(k);
            }
        }
        assert_eq!(got, vec![100, 1, 2]);
    }
}

#[cfg(test)]
mod live_tests {
    #[test]
    #[ignore]
    fn github_release_asset() {
        let u = "https://github.com/goglesquirmintontheiii/us-explorer/releases/download/v0.0.0/US_Explorer-0.0.0-x86_64.AppImage.sha256";
        let r = super::get(u);
        eprintln!("{:?}", r.as_ref().map(|b| String::from_utf8_lossy(b).to_string()));
        let r2 = super::get_with(u, "application/octet-stream", |_, _| {});
        eprintln!("octet: {:?}", r2.as_ref().map(|b| b.len()));
    }
}
