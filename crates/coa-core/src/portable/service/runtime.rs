//! The Host runtime of the application: one thread that owns the [`PortableService`], looks at the realms every second, and does what the
//! interface asks, one thing at a time. The interface never touches a store: it asks for an action and reads the published state.
//!
//! The thread starts with the application and stops with it. Sessions are persisted by the stores, so a Manager that was closed (or killed)
//! while a character was in play simply continues at its next start: the realm's own logout save, the baseline and every acknowledged
//! checkpoint are in the stores, the rest is read from the realm again.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::engine::{PortableService, ServerControl, ServiceError};
use super::view::PortableState;

type Job = Box<dyn FnOnce(&mut PortableService) + Send>;

enum Message {
    Run(Job),
    Stop,
}

pub struct PortableRuntime {
    tx: Sender<Message>,
    state: Arc<Mutex<PortableState>>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

pub type Installs = Box<dyn Fn() -> Vec<(String, PathBuf)> + Send>;

impl PortableRuntime {
    pub fn start(
        dir: &Path,
        installs: Installs,
        control: Option<Box<dyn ServerControl>>,
    ) -> Result<PortableRuntime, ServiceError> {
        let mut service = PortableService::open(dir, installs())?;
        if let Some(c) = control {
            service.set_control(c);
        }
        let state = Arc::new(Mutex::new(PortableState::default()));
        service.set_publisher(state.clone());
        let (tx, rx) = mpsc::channel::<Message>();
        let thread = std::thread::Builder::new()
            .name("portable-host".into())
            .spawn(move || {
                let mut last_installs = Instant::now();
                let mut last_tick = Instant::now() - Duration::from_secs(1);
                loop {
                    match rx.recv_timeout(Duration::from_millis(500)) {
                        Ok(Message::Run(job)) => job(&mut service),
                        Ok(Message::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                        Err(RecvTimeoutError::Timeout) => {}
                    }
                    if last_installs.elapsed() >= Duration::from_secs(5) {
                        service.set_installs(installs());
                        last_installs = Instant::now();
                    }
                    if last_tick.elapsed() >= Duration::from_secs(1) {
                        last_tick = Instant::now();
                        service.tick();
                    }
                }
            })
            .map_err(|e| ServiceError {
                code: "storage".into(),
                message: e.to_string(),
                notes: vec![],
            })?;
        Ok(PortableRuntime {
            tx,
            state,
            thread: Mutex::new(Some(thread)),
        })
    }

    /// The state as the thread last published it. Never waits for an operation.
    pub fn state(&self) -> PortableState {
        self.state.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Run something on the thread and wait for its answer.
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut PortableService) -> R + Send + 'static,
    ) -> Result<R, ServiceError> {
        let (back, answer) = mpsc::channel();
        self.tx
            .send(Message::Run(Box::new(move |s| {
                let _ = back.send(f(s));
            })))
            .map_err(|_| ServiceError {
                code: "stopped".into(),
                message: "The portable play service is not running.".into(),
                notes: vec![],
            })?;
        answer.recv().map_err(|_| ServiceError {
            code: "stopped".into(),
            message: "The portable play service stopped while it was working.".into(),
            notes: vec![],
        })
    }

    /// Stop the thread and wait for it. What it was doing is finished first; nothing is cut off mid-write.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Message::Stop);
        if let Some(t) = self.thread.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.join();
        }
    }
}

impl Drop for PortableRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}
