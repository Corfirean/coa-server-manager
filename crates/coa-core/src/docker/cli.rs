//! The one place that runs the `docker` command line. Everything else talks to the `Docker` trait, so the logic can
//! be tested without Docker. Only the `docker` client is needed on the computer (no `docker compose`).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::error::{Error, Result};

#[derive(Debug, Clone, Default)]
pub struct Output {
    /// None when the command timed out.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn ok(&self) -> bool {
        self.code == Some(0)
    }

    /// stdout and stderr, trimmed, for the technical details shown to the user.
    pub fn text(&self) -> String {
        format!("{}\n{}", self.stdout.trim(), self.stderr.trim())
            .trim()
            .to_string()
    }
}

/// What to run. `env` is where secrets go: a container option `-e NAME` (without a value) copies it from the
/// environment of the docker client, so a password never appears on a command line.
pub struct Call<'a> {
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub stdin: Option<&'a [u8]>,
    pub timeout: Duration,
}

impl<'a> Call<'a> {
    pub fn new(args: &[&str], timeout: Duration) -> Call<'a> {
        Call {
            args: args.iter().map(|a| a.to_string()).collect(),
            env: Vec::new(),
            stdin: None,
            timeout,
        }
    }
}

pub trait Docker {
    /// Err only when `docker` cannot be started at all (not installed).
    fn run(&self, call: &Call) -> Result<Output>;

    /// Is a server really listening on this address? Docker's own proxy accepts connections on a published port even
    /// when nothing listens inside the container, and closes them at once; so a connection that stays open (or gets
    /// data, as the world server's greeting) is the proof, a connect alone is not.
    fn port_open(&self, addr: SocketAddr) -> bool {
        let Ok(mut s) = TcpStream::connect_timeout(&addr, Duration::from_millis(400)) else {
            return false;
        };
        let _ = s.set_read_timeout(Some(Duration::from_millis(300)));
        match s.read(&mut [0u8; 1]) {
            Ok(0) => false,
            Ok(_) => true,
            Err(e) => matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ),
        }
    }

    fn pause(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

pub struct SystemDocker;

impl Docker for SystemDocker {
    fn run(&self, call: &Call) -> Result<Output> {
        let mut cmd = Command::new("docker");
        cmd.args(&call.args)
            .envs(call.env.iter().map(|(k, v)| (k, v)))
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        cmd.stdin(if call.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        }
        let mut child = cmd
            .spawn()
            .map_err(|e| Error::Invalid(format!("docker could not be started: {e}")))?;
        if let (Some(data), Some(mut stdin)) = (call.stdin, child.stdin.take()) {
            // Closing stdin at the end of this block tells docker the input is complete.
            let _ = stdin.write_all(data);
        }
        let mut out = child.stdout.take().expect("piped");
        let mut err = child.stderr.take().expect("piped");
        let out_t = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = out.read_to_string(&mut s);
            s
        });
        let err_t = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });
        let deadline = Instant::now() + call.timeout;
        let code = loop {
            if let Some(st) = child.try_wait()? {
                break st.code();
            }
            if Instant::now() > deadline {
                // Only the docker client is stopped; the containers it started keep running.
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        Ok(Output {
            code,
            stdout: out_t.join().unwrap_or_default(),
            stderr: err_t.join().unwrap_or_default(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_published_port_with_nothing_behind_it_does_not_count_as_open() {
        let sys = SystemDocker;
        // Closed port.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_addr = closed.local_addr().unwrap();
        drop(closed);
        assert!(!sys.port_open(closed_addr));
        // Like docker-proxy without a backend: accepts, then closes at once.
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_addr = proxy.local_addr().unwrap();
        let t = std::thread::spawn(move || drop(proxy.accept().unwrap()));
        assert!(!sys.port_open(proxy_addr));
        t.join().unwrap();
        // A server that waits for the client (auth server) and one that greets first (world server).
        let quiet = TcpListener::bind("127.0.0.1:0").unwrap();
        let quiet_addr = quiet.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (_s, _) = quiet.accept().unwrap();
            std::thread::sleep(Duration::from_millis(700));
        });
        assert!(sys.port_open(quiet_addr));
        t.join().unwrap();
        let greeter = TcpListener::bind("127.0.0.1:0").unwrap();
        let greeter_addr = greeter.local_addr().unwrap();
        let t = std::thread::spawn(move || {
            let (mut s, _) = greeter.accept().unwrap();
            s.write_all(b"hello").unwrap();
            std::thread::sleep(Duration::from_millis(400));
        });
        assert!(sys.port_open(greeter_addr));
        t.join().unwrap();
    }
}
