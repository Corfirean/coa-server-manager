//! Direct Ingress Proxy for Phase 14:
//! Client connects to direct public TCP -> Host direct ingress/proxy
//!   ├─ AUTH  -> 127.0.0.1:3724 (with narrow CMD_REALM_LIST address rewrite)
//!   └─ WORLD -> 127.0.0.1:8085 (dumb byte tunnel)
//!
//! Reuses `coa_control_proto::relay::rewrite_realm_list_address` so direct Internet players,
//! Relay players, and LAN players coexist without mutating AzerothCore's MySQL realmlist.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::error::{Error, Result};

pub struct DirectIngress {
    auth_port: u16,
    world_port: u16,
    world_target: Arc<std::sync::RwLock<String>>,
    stop: Arc<AtomicBool>,
    #[allow(dead_code)]
    threads: Vec<JoinHandle<()>>,
}

impl DirectIngress {
    pub fn start(
        auth_bind_port: u16,
        world_bind_port: u16,
        public_ip: String,
        ext_world_port: u16,
        local_auth_port: u16,
        local_world_port: u16,
    ) -> Result<Self> {
        let auth_listener =
            TcpListener::bind(format!("0.0.0.0:{auth_bind_port}")).map_err(|e| {
                Error::Invalid(format!(
                    "Cannot bind direct auth listener on port {auth_bind_port}: {e}"
                ))
            })?;
        auth_listener
            .set_nonblocking(true)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let auth_actual_port = auth_listener
            .local_addr()
            .map_err(|e| Error::Invalid(e.to_string()))?
            .port();

        let world_listener =
            TcpListener::bind(format!("0.0.0.0:{world_bind_port}")).map_err(|e| {
                Error::Invalid(format!(
                    "Cannot bind direct world listener on port {world_bind_port}: {e}"
                ))
            })?;
        world_listener
            .set_nonblocking(true)
            .map_err(|e| Error::Invalid(e.to_string()))?;
        let world_actual_port = world_listener
            .local_addr()
            .map_err(|e| Error::Invalid(e.to_string()))?
            .port();

        let stop = Arc::new(AtomicBool::new(false));
        let mut threads = Vec::new();

        let effective_world_port = if ext_world_port == 0 {
            world_actual_port
        } else {
            ext_world_port
        };
        let world_target = Arc::new(std::sync::RwLock::new(format!(
            "{public_ip}:{effective_world_port}"
        )));

        // Spawn Auth listener thread
        let stop_auth = stop.clone();
        let target_clone = world_target.clone();
        let auth_handle = std::thread::Builder::new()
            .name("direct-ingress-auth".into())
            .spawn(move || {
                while !stop_auth.load(Ordering::Relaxed) {
                    match auth_listener.accept() {
                        Ok((client_stream, _)) => {
                            let _ = client_stream.set_nonblocking(false);
                            let target = target_clone.clone();
                            std::thread::Builder::new()
                                .name("direct-auth-worker".into())
                                .spawn(move || {
                                    let _ =
                                        handle_auth_client(client_stream, local_auth_port, target);
                                })
                                .ok();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| Error::Invalid(e.to_string()))?;
        threads.push(auth_handle);

        // Spawn World listener thread
        let stop_world = stop.clone();
        let world_handle = std::thread::Builder::new()
            .name("direct-ingress-world".into())
            .spawn(move || {
                while !stop_world.load(Ordering::Relaxed) {
                    match world_listener.accept() {
                        Ok((client_stream, _)) => {
                            let _ = client_stream.set_nonblocking(false);
                            std::thread::Builder::new()
                                .name("direct-world-worker".into())
                                .spawn(move || {
                                    let _ = handle_world_client(client_stream, local_world_port);
                                })
                                .ok();
                        }
                        Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(20));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| Error::Invalid(e.to_string()))?;
        threads.push(world_handle);

        Ok(Self {
            auth_port: auth_actual_port,
            world_port: world_actual_port,
            world_target,
            stop,
            threads,
        })
    }

    pub fn auth_port(&self) -> u16 {
        self.auth_port
    }

    pub fn world_port(&self) -> u16 {
        self.world_port
    }

    pub fn update_external_world_endpoint(&self, public_ip: &str, mapped_world_port: u16) {
        if let Ok(mut g) = self.world_target.write() {
            *g = format!("{public_ip}:{mapped_world_port}");
        }
    }

    pub fn world_target(&self) -> String {
        self.world_target
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for DirectIngress {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_auth_client(
    mut client: TcpStream,
    local_auth_port: u16,
    world_target: Arc<std::sync::RwLock<String>>,
) -> Result<()> {
    let mut server = TcpStream::connect(format!("127.0.0.1:{local_auth_port}"))
        .map_err(|e| Error::Invalid(format!("Cannot connect to local authserver: {e}")))?;

    let _ = client.set_read_timeout(Some(Duration::from_millis(500)));
    let _ = server.set_read_timeout(Some(Duration::from_millis(500)));

    let mut client_read = client
        .try_clone()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut server_write = server
        .try_clone()
        .map_err(|e| Error::Invalid(e.to_string()))?;

    let done = Arc::new(AtomicBool::new(false));
    let done_c = done.clone();
    let server_shutdown = server.try_clone().ok();

    // Forward Client -> Auth Server
    let t_c2s = std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while !done_c.load(Ordering::Relaxed) {
            match client_read.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if server_write.write_all(&buf[..n]).is_err() {
                        break;
                    }
                }
                Err(ref e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        || e.kind() == std::io::ErrorKind::TimedOut =>
                {
                    continue;
                }
                Err(_) => break,
            }
        }
        done_c.store(true, Ordering::Relaxed);
        if let Some(s) = server_shutdown {
            let _ = s.shutdown(std::net::Shutdown::Both);
        }
    });

    // Forward Auth Server -> Client with narrow CMD_REALM_LIST rewrite
    let client_shutdown = client.try_clone().ok();
    let mut buf = [0u8; 8192];
    while !done.load(Ordering::Relaxed) {
        match server.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let chunk = &buf[..n];
                let current_target = world_target
                    .read()
                    .map(|s| s.clone())
                    .unwrap_or_else(|_| "127.0.0.1:8085".to_string());
                match coa_control_proto::relay::rewrite_realm_list_address(chunk, &current_target) {
                    Ok(Some(rewritten)) => {
                        if client.write_all(&rewritten).is_err() {
                            break;
                        }
                    }
                    _ => {
                        if client.write_all(chunk).is_err() {
                            break;
                        }
                    }
                }
            }
            Err(ref e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(_) => break,
        }
    }
    done.store(true, Ordering::Relaxed);
    if let Some(c) = client_shutdown {
        let _ = c.shutdown(std::net::Shutdown::Write);
    }

    let _ = t_c2s.join();
    Ok(())
}

fn handle_world_client(mut client: TcpStream, local_world_port: u16) -> Result<()> {
    let mut server = TcpStream::connect(format!("127.0.0.1:{local_world_port}"))
        .map_err(|e| Error::Invalid(format!("Cannot connect to local worldserver: {e}")))?;

    let _ = client.set_nonblocking(false);
    let _ = server.set_nonblocking(false);
    let _ = client.set_read_timeout(None);
    let _ = server.set_read_timeout(None);

    let mut client_read = client
        .try_clone()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut server_write = server
        .try_clone()
        .map_err(|e| Error::Invalid(e.to_string()))?;

    let t_c2s = std::thread::spawn(move || {
        let _ = std::io::copy(&mut client_read, &mut server_write);
        let _ = server_write.shutdown(std::net::Shutdown::Both);
    });

    let _ = std::io::copy(&mut server, &mut client);
    let _ = client.shutdown(std::net::Shutdown::Both);

    let _ = t_c2s.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_direct_ingress_auth_and_world() {
        let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
        let mock_auth_port = mock_auth.local_addr().unwrap().port();

        let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
        let mock_world_port = mock_world.local_addr().unwrap().port();

        let mut ingress = DirectIngress::start(
            0,
            0,
            "198.51.100.1".to_string(),
            8085,
            mock_auth_port,
            mock_world_port,
        )
        .unwrap();

        let ingress_auth = ingress.auth_port();
        let ingress_world = ingress.world_port();

        // 1. Verify World dumb byte tunnel
        let t_world = std::thread::spawn(move || {
            let (mut stream, _) = mock_world.accept().unwrap();
            let mut buf = [0u8; 16];
            let n = stream.read(&mut buf).unwrap();
            assert_eq!(&buf[..n], b"WORLD_PING");
            stream.write_all(b"WORLD_PONG").unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        });

        let mut client_world = TcpStream::connect(format!("127.0.0.1:{ingress_world}")).unwrap();
        client_world.write_all(b"WORLD_PING").unwrap();
        let mut resp = [0u8; 16];
        let n = client_world.read(&mut resp).unwrap();
        assert_eq!(&resp[..n], b"WORLD_PONG");
        drop(client_world);
        t_world.join().unwrap();

        // 2. Verify Auth rewrite
        let t_auth = std::thread::spawn(move || {
            let (mut stream, _) = mock_auth.accept().unwrap();
            let mut buf = [0u8; 16];
            let _ = stream.read(&mut buf).unwrap();

            let mut fake_realm_list = vec![
                0x10, // Opcode CMD_REALM_LIST
                0x00, 0x00, // Size placeholder
                0x00, 0x00, 0x00, 0x00, // Unused
                0x01, 0x00, // Realm count: 1
                0x01, 0x00, 0x00, 0x00, // Icon
                0x00, // Lock
                0x00, // Flags
            ];
            fake_realm_list.extend_from_slice(b"RealmOne\0");
            fake_realm_list.extend_from_slice(b"127.0.0.1:8085\0"); // original target
            fake_realm_list.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]); // Population
            fake_realm_list.extend_from_slice(&[0x01]); // Num characters
            fake_realm_list.extend_from_slice(&[0x01]); // Timezone
            fake_realm_list.extend_from_slice(&[0x00]); // Realm ID
            fake_realm_list.extend_from_slice(&[0x02, 0x00]); // Footer

            let body_len = (fake_realm_list.len() - 3) as u16;
            fake_realm_list[1..3].copy_from_slice(&body_len.to_le_bytes());

            stream.write_all(&fake_realm_list).unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        });

        let mut client_auth = TcpStream::connect(format!("127.0.0.1:{ingress_auth}")).unwrap();
        client_auth.write_all(b"AUTH_HELLO").unwrap();
        let mut auth_buf = [0u8; 512];
        let an = client_auth.read(&mut auth_buf).unwrap();
        let payload = String::from_utf8_lossy(&auth_buf[..an]);
        assert!(
            payload.contains("198.51.100.1:8085"),
            "Expected rewritten world address, got: {payload}"
        );
        drop(client_auth);
        t_auth.join().unwrap();

        ingress.stop();
    }

    #[test]
    fn test_direct_ingress_uses_final_mapped_world_port_not_desired_or_bound() {
        let desired_world_port = 8085;
        let mock_auth = TcpListener::bind("127.0.0.1:0").unwrap();
        let mock_auth_port = mock_auth.local_addr().unwrap().port();

        let mock_world = TcpListener::bind("127.0.0.1:0").unwrap();
        let mock_world_port = mock_world.local_addr().unwrap().port();

        // DirectIngress binds to ephemeral ports (0, 0), so actual ingress port != 8085
        let ingress = DirectIngress::start(
            0,
            0,
            "198.51.100.1".to_string(),
            desired_world_port,
            mock_auth_port,
            mock_world_port,
        )
        .unwrap();

        let actual_ingress_world = ingress.world_port();
        assert_ne!(
            actual_ingress_world, desired_world_port,
            "Actual ingress port must not be 8085"
        );

        // The router / NAT-PMP assigns an external mapped world port != 8085 and != actual ingress port
        let external_mapped_world_port = 48085;
        assert_ne!(external_mapped_world_port, desired_world_port);
        assert_ne!(external_mapped_world_port, actual_ingress_world);

        // Update with final verified mapped external endpoint: public_ip:mapped_world_port
        ingress.update_external_world_endpoint("198.51.100.1", external_mapped_world_port);

        let t_auth = std::thread::spawn(move || {
            let (mut stream, _) = mock_auth.accept().unwrap();
            let mut buf = [0u8; 16];
            let _ = stream.read(&mut buf).unwrap();

            let mut fake_realm_list = vec![
                0x10, // Opcode CMD_REALM_LIST
                0x00, 0x00, // Size placeholder
                0x00, 0x00, 0x00, 0x00, // Unused
                0x01, 0x00, // Realm count: 1
                0x01, 0x00, 0x00, 0x00, // Icon
                0x00, // Lock
                0x00, // Flags
            ];
            fake_realm_list.extend_from_slice(b"RealmOne\0");
            fake_realm_list.extend_from_slice(b"127.0.0.1:8085\0");
            fake_realm_list.extend_from_slice(&[0x00, 0x00, 0x00, 0x00]);
            fake_realm_list.extend_from_slice(&[0x01]);
            fake_realm_list.extend_from_slice(&[0x01]);
            fake_realm_list.extend_from_slice(&[0x00]);
            fake_realm_list.extend_from_slice(&[0x02, 0x00]);

            let body_len = (fake_realm_list.len() - 3) as u16;
            fake_realm_list[1..3].copy_from_slice(&body_len.to_le_bytes());

            stream.write_all(&fake_realm_list).unwrap();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        });

        let mut client_auth =
            TcpStream::connect(format!("127.0.0.1:{}", ingress.auth_port())).unwrap();
        client_auth.write_all(b"AUTH_HELLO").unwrap();
        let mut auth_buf = [0u8; 512];
        let an = client_auth.read(&mut auth_buf).unwrap();
        let payload = String::from_utf8_lossy(&auth_buf[..an]);

        // Assert that the rewritten REALM_LIST contains EXACTLY the mapped external port
        assert!(
            payload.contains("198.51.100.1:48085"),
            "Expected rewritten realm list to contain mapped external port 48085, got: {payload}"
        );
        assert!(
            !payload.contains("198.51.100.1:8085"),
            "Must NOT contain desired world port 8085"
        );
        assert!(
            !payload.contains(&format!("198.51.100.1:{actual_ingress_world}")),
            "Must NOT contain actual internal ingress port"
        );

        drop(client_auth);
        t_auth.join().unwrap();
    }
}
