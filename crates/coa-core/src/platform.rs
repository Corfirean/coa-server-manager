//! Which kind of server a new installation is on this computer. This is the one place where the host decides: a Windows
//! computer installs a repack, anything else installs a Docker server. The screens and the commands ask here and then
//! call the code of that kind, so no platform check is scattered through shared code.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Flavor {
    /// The CoA Repack: Windows executables supervised by the repack's launcher.
    Repack,
    /// MySQL, the auth server and the world server in Docker containers.
    Docker,
}

pub fn flavor() -> Flavor {
    if cfg!(windows) {
        Flavor::Repack
    } else {
        Flavor::Docker
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_host_decides_once() {
        #[cfg(windows)]
        assert_eq!(flavor(), Flavor::Repack);
        #[cfg(not(windows))]
        assert_eq!(flavor(), Flavor::Docker);
    }

    #[test]
    fn the_screens_read_the_flavor_as_a_plain_word() {
        assert_eq!(serde_json::to_string(&Flavor::Repack).unwrap(), "\"repack\"");
        assert_eq!(serde_json::to_string(&Flavor::Docker).unwrap(), "\"docker\"");
    }
}
