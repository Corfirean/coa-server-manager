//! Where a problem report goes. A report about the bots goes to the repository of the bot system in question, anything
//! else to the Manager's own. The repositories come from the module catalog, so there is one list to keep right.

use std::path::Path;

use serde::Serialize;

pub const MANAGER_REPO: &str = "Corfirean/coa-server-manager";

/// One place a report can be filed. `id` is what the page sends back; `repo` is `owner/name` on GitHub.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Target {
    pub id: &'static str,
    pub repo: String,
    /// The report form asks for the Manager's labels only where we own the repository.
    pub ours: bool,
}

/// `https://github.com/owner/name/tree/main/x` -> `owner/name`.
fn repo_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://github.com/")?;
    let mut parts = rest.split('/');
    let (owner, name) = (parts.next()?, parts.next()?);
    let ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
    };
    (ok(owner) && ok(name)).then(|| format!("{owner}/{name}"))
}

fn catalog_repo(id: &str) -> Option<String> {
    crate::modules::catalog()
        .into_iter()
        .find(|e| e.id == id)
        .and_then(|e| repo_of(&e.repo))
}

/// The three places, in the order the page lists them. A bot system whose repository is unknown falls back to the Manager's.
pub fn targets() -> Vec<Target> {
    let mut v = vec![Target {
        id: "manager",
        repo: MANAGER_REPO.into(),
        ours: true,
    }];
    for (id, module) in [("companions", "companions"), ("squid", "playerbots")] {
        if let Some(repo) = catalog_repo(module) {
            v.push(Target {
                id,
                repo: repo.clone(),
                ours: repo.starts_with("Corfirean/"),
            });
        }
    }
    v
}

pub fn target(id: &str) -> Option<Target> {
    targets().into_iter().find(|t| t.id == id)
}

/// True for the "new issue" page of one of the targets: exactly that page, optionally with a query.
pub fn is_new_issue_url(url: &str) -> bool {
    targets().iter().any(|t| {
        let base = format!("https://github.com/{}/issues/new", t.repo);
        url == base || url.starts_with(&format!("{base}?"))
    })
}

/// What the report form fills in about the bot systems of this server.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Bots {
    /// The target the form should start with: the bot system that is switched on, else the Manager.
    pub suggested: &'static str,
    /// The release of SQUID Playerbots, when it is the one switched on.
    pub squid_version: Option<String>,
}

pub fn bots(root: &Path) -> Bots {
    let list = crate::modules::list(root);
    let on = |id: &str| {
        list.iter()
            .find(|m| m.id == id)
            .filter(|m| m.installed && m.enabled)
    };
    let squid = on("playerbots");
    Bots {
        suggested: if squid.is_some() {
            "squid"
        } else if on("companions").is_some() {
            "companions"
        } else {
            "manager"
        },
        squid_version: squid
            .and_then(|m| m.version.clone())
            .filter(|v| !v.is_empty()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_targets_come_from_the_catalog() {
        let t = targets();
        let repo = |id: &str| t.iter().find(|x| x.id == id).map(|x| x.repo.clone());
        assert_eq!(
            repo("manager").as_deref(),
            Some("Corfirean/coa-server-manager")
        );
        assert_eq!(
            repo("companions").as_deref(),
            Some("Corfirean/mod-coa-playerbots")
        );
        assert_eq!(repo("squid").as_deref(), Some("Zyth45/mod-playerbots"));
        assert!(t.iter().find(|x| x.id == "squid").is_some_and(|x| !x.ours));
    }

    #[test]
    fn repository_names_are_read_from_module_links() {
        assert_eq!(
            repo_of("https://github.com/Corfirean/azerothcore-wotlk-coa/tree/coa-bots/modules/x")
                .as_deref(),
            Some("Corfirean/azerothcore-wotlk-coa")
        );
        assert_eq!(
            repo_of("https://github.com/Zyth45/mod-playerbots").as_deref(),
            Some("Zyth45/mod-playerbots")
        );
        assert_eq!(repo_of("https://example.com/a/b"), None);
        assert_eq!(repo_of("https://github.com/a"), None);
    }

    #[test]
    fn only_the_new_issue_pages_of_the_targets_are_allowed() {
        assert!(is_new_issue_url(
            "https://github.com/Zyth45/mod-playerbots/issues/new"
        ));
        assert!(is_new_issue_url(
            "https://github.com/Corfirean/mod-coa-playerbots/issues/new?title=a&body=b"
        ));
        assert!(!is_new_issue_url(
            "https://github.com/Zyth45/mod-playerbots/issues/newer"
        ));
        assert!(!is_new_issue_url(
            "https://github.com/Zyth45/other/issues/new"
        ));
        assert!(!is_new_issue_url(
            "https://evil.example/Corfirean/coa-server-manager/issues/new"
        ));
    }

    #[test]
    fn the_suggested_target_follows_the_bot_system_that_is_on() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("srv");
        crate::layout::testkit::fake_repack(&root);
        let conf = root.join("Core/configs/modules");
        std::fs::write(conf.join("mod_coa_playerbots.conf"), "CoaBots.Enable = 1\n").unwrap();
        assert_eq!(bots(&root).suggested, "companions");
        std::fs::write(conf.join("mod_coa_playerbots.conf"), "CoaBots.Enable = 0\n").unwrap();
        assert_eq!(bots(&root).suggested, "manager");
    }
}
