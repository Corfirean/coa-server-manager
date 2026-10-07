//! The publishing loop on its own thread: it starts with the application, keeps going while the interface is elsewhere, and stops with the Manager.
//! Commands are answered between passes; a pass blocks only on the Registry's own timeouts.

use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::advert::AdvertSource;
use super::host::{system_clock, Clock, RegistryHost, RegistryStatus, Timing};
use super::keys::KeyStore;
use crate::{Error, Result};

enum Cmd {
    SetUrl(Option<String>, Sender<Result<()>>),
    Publish { local_id: String, name: String, description: String, language: String, region: Option<String>, reply: Sender<Result<()>> },
    Unpublish(String, Sender<Result<()>>),
    Retry(String),
    Shutdown,
}

pub struct RegistryRuntime {
    tx: Sender<Cmd>,
    status: Arc<Mutex<RegistryStatus>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl RegistryRuntime {
    pub fn start(dir: &Path, keys: Arc<dyn KeyStore>, source: Arc<dyn AdvertSource>) -> Result<Self> {
        Self::start_with(dir, keys, source, system_clock(), Timing::default())
    }

    pub fn start_with(dir: &Path, keys: Arc<dyn KeyStore>, source: Arc<dyn AdvertSource>, clock: Clock, timing: Timing) -> Result<Self> {
        let mut host = RegistryHost::open(dir, keys, source, clock, timing, Instant::now())?;
        let status = Arc::new(Mutex::new(host.status(Instant::now())));
        let (tx, rx) = mpsc::channel::<Cmd>();
        let shared = status.clone();
        let join = std::thread::Builder::new()
            .name("realm-registry".into())
            .spawn(move || loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                    Ok(Cmd::SetUrl(url, reply)) => {
                        let _ = reply.send(host.set_url(url.as_deref(), Instant::now()));
                    }
                    Ok(Cmd::Publish { local_id, name, description, language, region, reply }) => {
                        let _ = reply.send(host.publish(&local_id, &name, &description, &language, region.as_deref(), Instant::now()));
                    }
                    Ok(Cmd::Unpublish(local_id, reply)) => {
                        let _ = reply.send(host.unpublish(&local_id, Instant::now()));
                    }
                    Ok(Cmd::Retry(local_id)) => host.retry(&local_id, Instant::now()),
                    Err(RecvTimeoutError::Timeout) => {}
                }
                let now = Instant::now();
                host.tick(now);
                if let Ok(mut s) = shared.lock() {
                    *s = host.status(now);
                }
            })
            .map_err(Error::Io)?;
        Ok(Self { tx, status, join: Mutex::new(Some(join)) })
    }

    pub fn status(&self) -> RegistryStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    fn ask(&self, make: impl FnOnce(Sender<Result<()>>) -> Cmd) -> Result<()> {
        let (reply, answer) = mpsc::channel();
        self.tx.send(make(reply)).map_err(|_| Error::Invalid("the registry loop has stopped".into()))?;
        answer.recv_timeout(Duration::from_secs(30)).map_err(|_| Error::Invalid("the registry loop did not answer".into()))?
    }

    pub fn set_url(&self, url: Option<String>) -> Result<()> {
        self.ask(|reply| Cmd::SetUrl(url, reply))
    }

    pub fn publish(&self, local_id: &str, name: &str, description: &str, language: &str, region: Option<&str>) -> Result<()> {
        self.ask(|reply| Cmd::Publish { local_id: local_id.into(), name: name.into(), description: description.into(), language: language.into(), region: region.map(str::to_string), reply })
    }

    pub fn unpublish(&self, local_id: &str) -> Result<()> {
        self.ask(|reply| Cmd::Unpublish(local_id.into(), reply))
    }

    pub fn retry(&self, local_id: &str) {
        let _ = self.tx.send(Cmd::Retry(local_id.into()));
    }

    /// Stop the loop. A published realm is left published: it simply stops being heard of and goes offline after the TTL.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Ok(mut guard) = self.join.lock() {
            if let Some(j) = guard.take() {
                let _ = j.join();
            }
        }
    }
}

impl Drop for RegistryRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}
