//! Extensions (Phase 7): how data of a game module this Manager has no built-in knowledge of travels with a character.
//!
//! * In the **canonical character** an extension is an opaque, hash-protected blob (`PortableCharacter::extensions`). It is never
//!   interpreted, edited or dropped by the Owner. A namespace nobody understands simply stays where it is.
//! * On a **realm**, an [`ExtensionAdapter`] is the only thing allowed to read the module's data out of the realm and to write the
//!   blob into it. An adapter belongs to one namespace (`mod:<module>`) and one range of format versions; whatever it does not
//!   claim is never applied, so a blob is never applied "blindly".
//! * Nothing is destroyed on the way. A realm without the module, or with another format of it, gets nothing, and the canonical blob
//!   stays for the next realm that can take it.
//!
//! No adapter for any real module exists yet; the framework is exercised by a fake test extension.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use super::capabilities::{ContentProfile, ExtensionSupport, FormatRange};
use super::error::{PortableError, Result};
use super::ids::valid_namespace;
use super::model::Extension;

/// How an adapter reaches a realm: one character, a way to ask, a way to write. The adapter owns its own SQL; the framework owns
/// nothing but the order of calls.
pub trait ExtensionRealm {
    fn local_guid(&self) -> u32;
    fn has_table(&self, table: &str) -> bool;
    fn query(&mut self, sql: &str) -> Result<String>;
    fn execute(&mut self, script: &str) -> Result<()>;
}

/// Whether one extension payload can be applied to a realm, as an explicit answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionCompatibility {
    Compatible,
    /// The realm does not have the module (or no adapter serves it): the payload stays canonical, the realm receives nothing.
    MissingOnRealm,
    /// The payload is older than the oldest format the realm understands.
    FormatTooOld {
        payload: u32,
        realm: FormatRange,
    },
    /// The payload is newer than the newest format the realm understands.
    FormatTooNew {
        payload: u32,
        realm: FormatRange,
    },
    /// The adapter refuses for its own reason.
    Incompatible(String),
}

impl ExtensionCompatibility {
    pub fn is_compatible(&self) -> bool {
        matches!(self, ExtensionCompatibility::Compatible)
    }
}

impl std::fmt::Display for ExtensionCompatibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExtensionCompatibility::Compatible => f.write_str("compatible"),
            ExtensionCompatibility::MissingOnRealm => {
                f.write_str("the realm does not have this module")
            }
            ExtensionCompatibility::FormatTooOld { payload, realm } => write!(
                f,
                "the payload is format {payload}, older than the {realm} the realm understands"
            ),
            ExtensionCompatibility::FormatTooNew { payload, realm } => write!(
                f,
                "the payload is format {payload}, newer than the {realm} the realm understands"
            ),
            ExtensionCompatibility::Incompatible(why) => write!(f, "incompatible: {why}"),
        }
    }
}

/// The adapter of one module.
pub trait ExtensionAdapter: Send + Sync {
    /// `mod:<module>`.
    fn namespace(&self) -> &str;
    fn module_version(&self) -> &str;
    /// The format versions of the payload this adapter reads and writes.
    fn supported_format_versions(&self) -> FormatRange;
    /// Is the module present on this realm (its tables exist, its version is the one this adapter is for)?
    fn available(&self, realm: &mut dyn ExtensionRealm) -> bool;
    /// Is this payload well formed? Called on everything an adapter exports and before it applies anything.
    fn validate(&self, extension: &Extension) -> Result<()>;
    /// Read the module's data of the character out of the realm, or `None` when the character has none.
    fn export(&self, realm: &mut dyn ExtensionRealm) -> Result<Option<Extension>>;
    /// Write a validated, compatible payload into the realm.
    fn apply(&self, realm: &mut dyn ExtensionRealm, extension: &Extension) -> Result<()>;
    /// Can this payload be applied to a realm with this content profile? The default is the format check against what the profile
    /// published; an adapter may be stricter.
    fn compatibility(
        &self,
        extension: &Extension,
        realm: &ContentProfile,
    ) -> ExtensionCompatibility {
        default_compatibility(self.namespace(), extension, realm)
    }
}

/// The format check every adapter starts from.
pub fn default_compatibility(
    namespace: &str,
    extension: &Extension,
    realm: &ContentProfile,
) -> ExtensionCompatibility {
    let Some(support) = realm.extension(namespace) else {
        return ExtensionCompatibility::MissingOnRealm;
    };
    if extension.format_version < support.formats.min {
        ExtensionCompatibility::FormatTooOld {
            payload: extension.format_version,
            realm: support.formats,
        }
    } else if extension.format_version > support.formats.max {
        ExtensionCompatibility::FormatTooNew {
            payload: extension.format_version,
            realm: support.formats,
        }
    } else {
        ExtensionCompatibility::Compatible
    }
}

/// What happened to one canonical extension on one realm.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionDisposition {
    /// Written to the realm now.
    Applied,
    /// Written before and unchanged since.
    AlreadyApplied,
    /// Not written, and why. The canonical payload is untouched.
    Deferred(ExtensionCompatibility),
    /// The adapter said yes but failed; nothing is claimed, the payload stays canonical and the next attempt starts again.
    Failed(String),
    /// Not a module extension (`coa:*` data the Manager itself keeps aside): never applied to a realm.
    CanonicalOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionOutcome {
    pub namespace: String,
    pub disposition: ExtensionDisposition,
}

impl std::fmt::Display for ExtensionOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.disposition {
            ExtensionDisposition::Applied => write!(f, "{}: applied", self.namespace),
            ExtensionDisposition::AlreadyApplied => {
                write!(f, "{}: already applied", self.namespace)
            }
            ExtensionDisposition::Deferred(why) => {
                write!(f, "{}: deferred, kept canonical ({why})", self.namespace)
            }
            ExtensionDisposition::Failed(why) => {
                write!(f, "{}: not applied, kept canonical ({why})", self.namespace)
            }
            ExtensionDisposition::CanonicalOnly => write!(f, "{}: canonical only", self.namespace),
        }
    }
}

/// Only module namespaces (`mod:...`) are ever handed to an adapter; the Manager's own `coa:*` blobs (quarantined settings) are
/// canonical-only.
pub fn is_module_namespace(namespace: &str) -> bool {
    namespace.starts_with("mod:") && valid_namespace(namespace)
}

#[derive(Default, Clone)]
pub struct ExtensionRegistry {
    adapters: BTreeMap<String, Arc<dyn ExtensionAdapter>>,
}

impl std::fmt::Debug for ExtensionRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.adapters.keys()).finish()
    }
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, adapter: Arc<dyn ExtensionAdapter>) -> Result<()> {
        let ns = adapter.namespace().to_string();
        if !is_module_namespace(&ns) {
            return Err(PortableError::Invalid(format!(
                "an extension namespace is mod:<module>, not {ns:?}"
            )));
        }
        let formats = adapter.supported_format_versions();
        if formats.min == 0
            || formats.min > formats.max
            || adapter.module_version().is_empty()
            || adapter.module_version().len() > 64
        {
            return Err(PortableError::Invalid(format!(
                "adapter {ns} declares an invalid version or format range"
            )));
        }
        if self.adapters.insert(ns.clone(), adapter).is_some() {
            return Err(PortableError::Invalid(format!("two adapters for {ns}")));
        }
        Ok(())
    }

    pub fn namespaces(&self) -> impl Iterator<Item = &str> {
        self.adapters.keys().map(String::as_str)
    }

    pub fn get(&self, namespace: &str) -> Option<&Arc<dyn ExtensionAdapter>> {
        self.adapters.get(namespace)
    }

    /// The extensions this realm can apply: the adapters whose module is present on it.
    pub fn supported_on(&self, realm: &mut dyn ExtensionRealm) -> Vec<ExtensionSupport> {
        self.adapters
            .values()
            .filter(|a| a.available(realm))
            .map(|a| ExtensionSupport {
                namespace: a.namespace().to_string(),
                module_version: a.module_version().to_string(),
                formats: a.supported_format_versions(),
            })
            .collect()
    }

    /// The explicit outcome for each extension of a canonical character on a realm with `profile`, without touching anything.
    pub fn evaluate(
        &self,
        extensions: &BTreeMap<String, Extension>,
        profile: &ContentProfile,
    ) -> Vec<ExtensionOutcome> {
        extensions
            .iter()
            .map(|(namespace, ext)| {
                let disposition = if !is_module_namespace(namespace) {
                    ExtensionDisposition::CanonicalOnly
                } else {
                    match self.adapters.get(namespace) {
                        None => {
                            ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm)
                        }
                        Some(adapter) => match adapter.compatibility(ext, profile) {
                            ExtensionCompatibility::Compatible => ExtensionDisposition::Applied,
                            other => ExtensionDisposition::Deferred(other),
                        },
                    }
                };
                ExtensionOutcome {
                    namespace: namespace.clone(),
                    disposition,
                }
            })
            .collect()
    }

    /// Read every supported extension of the character out of the realm. A payload an adapter produces is validated before it
    /// leaves; an adapter that fails or produces rubbish contributes nothing and the canonical payload is left alone.
    pub fn export_all(
        &self,
        realm: &mut dyn ExtensionRealm,
    ) -> (BTreeMap<String, Extension>, Vec<String>) {
        let mut out = BTreeMap::new();
        let mut problems = Vec::new();
        for (namespace, adapter) in &self.adapters {
            if !adapter.available(realm) {
                continue;
            }
            match adapter.export(realm) {
                Ok(Some(ext)) => match check_exported(adapter.as_ref(), &ext) {
                    Ok(()) => {
                        out.insert(namespace.clone(), ext);
                    }
                    Err(e) => problems.push(format!(
                        "{namespace}: the exported payload is not valid ({e})"
                    )),
                },
                Ok(None) => {}
                Err(e) => problems.push(format!("{namespace}: export failed ({e})")),
            }
        }
        (out, problems)
    }

    /// Apply what can be applied. `applied` is what was applied before (namespace -> content hash): an unchanged payload is not applied
    /// twice. Returns one outcome per canonical extension, in namespace order.
    pub fn apply_all(
        &self,
        realm: &mut dyn ExtensionRealm,
        extensions: &BTreeMap<String, Extension>,
        profile: &ContentProfile,
        applied: &HashMap<String, String>,
    ) -> Vec<ExtensionOutcome> {
        extensions
            .iter()
            .map(|(namespace, ext)| {
                let disposition = if !is_module_namespace(namespace) {
                    ExtensionDisposition::CanonicalOnly
                } else if let Some(adapter) = self.adapters.get(namespace) {
                    self.apply_one(
                        adapter.as_ref(),
                        realm,
                        ext,
                        profile,
                        applied.get(namespace.as_str()),
                    )
                } else {
                    ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm)
                };
                ExtensionOutcome {
                    namespace: namespace.clone(),
                    disposition,
                }
            })
            .collect()
    }

    fn apply_one(
        &self,
        adapter: &dyn ExtensionAdapter,
        realm: &mut dyn ExtensionRealm,
        ext: &Extension,
        profile: &ContentProfile,
        applied: Option<&String>,
    ) -> ExtensionDisposition {
        if !adapter.available(realm) {
            return ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm);
        }
        let compatibility = adapter.compatibility(ext, profile);
        if !compatibility.is_compatible() {
            return ExtensionDisposition::Deferred(compatibility);
        }
        if applied.is_some_and(|h| *h == ext.content_hash) {
            return ExtensionDisposition::AlreadyApplied;
        }
        if !ext.is_intact() {
            return ExtensionDisposition::Failed(
                "the payload does not match its content hash".into(),
            );
        }
        if let Err(e) = adapter.validate(ext) {
            return ExtensionDisposition::Failed(format!("the payload is not valid: {e}"));
        }
        match adapter.apply(realm, ext) {
            Ok(()) => ExtensionDisposition::Applied,
            Err(e) => ExtensionDisposition::Failed(e.to_string()),
        }
    }
}

fn check_exported(adapter: &dyn ExtensionAdapter, ext: &Extension) -> Result<()> {
    if !ext.is_intact() {
        return Err(PortableError::CorruptSnapshot(
            "the payload does not match its content hash".into(),
        ));
    }
    if !adapter
        .supported_format_versions()
        .contains(ext.format_version)
    {
        return Err(PortableError::Invalid(format!(
            "format {} is outside what the adapter declares",
            ext.format_version
        )));
    }
    adapter.validate(ext)
}

/// An adapter's realm over the Manager's database connection: one character of one realm.
pub struct DbExtensionRealm<'a> {
    db: &'a crate::db::Db,
    guid: u32,
    tables: &'a super::realm::SchemaProbe,
}

impl<'a> DbExtensionRealm<'a> {
    pub fn new(db: &'a crate::db::Db, guid: u32, tables: &'a super::realm::SchemaProbe) -> Self {
        Self { db, guid, tables }
    }
}

impl ExtensionRealm for DbExtensionRealm<'_> {
    fn local_guid(&self) -> u32 {
        self.guid
    }
    fn has_table(&self, table: &str) -> bool {
        self.tables.has(table)
    }
    fn query(&mut self, sql: &str) -> Result<String> {
        self.db
            .query(sql)
            .map_err(|e| PortableError::RealmRead(e.to_string()))
    }
    fn execute(&mut self, script: &str) -> Result<()> {
        self.db
            .query(script)
            .map(|_| ())
            .map_err(|e| PortableError::RealmRead(e.to_string()))
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A fake module for the tests of the framework: its data is one blob per character, kept in a map.

    use std::sync::Mutex;

    use super::*;

    /// A fake realm: the module's table, per character.
    #[derive(Default)]
    pub struct FakeExtensionRealm {
        pub guid: u32,
        pub has_module: bool,
        pub data: BTreeMap<u32, Vec<u8>>,
        pub writes: u32,
    }

    impl ExtensionRealm for FakeExtensionRealm {
        fn local_guid(&self) -> u32 {
            self.guid
        }
        fn has_table(&self, table: &str) -> bool {
            self.has_module && table == "mod_fake"
        }
        fn query(&mut self, sql: &str) -> Result<String> {
            assert_eq!(sql, "GET");
            Ok(self
                .data
                .get(&self.guid)
                .map(hex::encode)
                .unwrap_or_default())
        }
        fn execute(&mut self, script: &str) -> Result<()> {
            let hex = script.strip_prefix("PUT ").expect("a fake write");
            self.data.insert(self.guid, hex::decode(hex).unwrap());
            self.writes += 1;
            Ok(())
        }
    }

    /// The fake module's adapter: formats 2..=3, a payload is at least one byte and at most 64.
    pub struct FakeAdapter {
        pub formats: FormatRange,
        pub version: &'static str,
        pub fail_apply: Mutex<bool>,
    }

    impl FakeAdapter {
        pub fn new(min: u32, max: u32) -> Arc<Self> {
            Arc::new(Self {
                formats: FormatRange::new(min, max),
                version: "1.0.0",
                fail_apply: Mutex::new(false),
            })
        }
    }

    impl ExtensionAdapter for FakeAdapter {
        fn namespace(&self) -> &str {
            "mod:fake"
        }
        fn module_version(&self) -> &str {
            self.version
        }
        fn supported_format_versions(&self) -> FormatRange {
            self.formats
        }
        fn available(&self, realm: &mut dyn ExtensionRealm) -> bool {
            realm.has_table("mod_fake")
        }
        fn validate(&self, extension: &Extension) -> Result<()> {
            if extension.payload.0.is_empty() || extension.payload.0.len() > 64 {
                return Err(PortableError::Invalid(
                    "a fake payload has 1 to 64 bytes".into(),
                ));
            }
            Ok(())
        }
        fn export(&self, realm: &mut dyn ExtensionRealm) -> Result<Option<Extension>> {
            let hex = realm.query("GET")?;
            if hex.is_empty() {
                return Ok(None);
            }
            Ok(Some(Extension::new(
                self.version,
                self.formats.max,
                hex::decode(hex).unwrap(),
            )))
        }
        fn apply(&self, realm: &mut dyn ExtensionRealm, extension: &Extension) -> Result<()> {
            if *self.fail_apply.lock().unwrap() {
                return Err(PortableError::RealmRead(
                    "the module's table is locked".into(),
                ));
            }
            realm.execute(&format!("PUT {}", hex::encode(&extension.payload.0)))
        }
    }

    pub fn payload(format: u32, bytes: &[u8]) -> Extension {
        Extension::new("1.0.0", format, bytes.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;
    use crate::portable::capabilities::{CharacterFormats, ContentProfile};
    use crate::portable::model::Ruleset;

    fn profile_with(extensions: Vec<ExtensionSupport>) -> ContentProfile {
        ContentProfile {
            ruleset: Ruleset::Coa,
            character_formats: CharacterFormats {
                readable: FormatRange::new(1, 2),
                writable: FormatRange::single(2),
            },
            online_import_job_formats: vec![2],
            session_protocol: 1,
            collection_protocol: 1,
            features: Default::default(),
            collection_kinds: Default::default(),
            extensions,
            client_catalog: Default::default(),
        }
    }

    fn registry(adapter: Arc<FakeAdapter>) -> ExtensionRegistry {
        let mut r = ExtensionRegistry::new();
        r.register(adapter).unwrap();
        r
    }

    fn realm(has_module: bool) -> FakeExtensionRealm {
        FakeExtensionRealm {
            guid: 7,
            has_module,
            ..Default::default()
        }
    }

    #[test]
    fn only_module_namespaces_with_a_sane_declaration_can_be_registered() {
        struct Odd(&'static str, FormatRange);
        impl ExtensionAdapter for Odd {
            fn namespace(&self) -> &str {
                self.0
            }
            fn module_version(&self) -> &str {
                "1"
            }
            fn supported_format_versions(&self) -> FormatRange {
                self.1
            }
            fn available(&self, _: &mut dyn ExtensionRealm) -> bool {
                true
            }
            fn validate(&self, _: &Extension) -> Result<()> {
                Ok(())
            }
            fn export(&self, _: &mut dyn ExtensionRealm) -> Result<Option<Extension>> {
                Ok(None)
            }
            fn apply(&self, _: &mut dyn ExtensionRealm, _: &Extension) -> Result<()> {
                Ok(())
            }
        }
        let mut r = ExtensionRegistry::new();
        assert!(
            r.register(Arc::new(Odd(
                "coa:unlisted-settings",
                FormatRange::single(1)
            )))
            .is_err(),
            "the Manager's own namespaces are not adapters"
        );
        assert!(r
            .register(Arc::new(Odd("fake", FormatRange::single(1))))
            .is_err());
        assert!(r
            .register(Arc::new(Odd("mod:zero", FormatRange::new(0, 1))))
            .is_err());
        assert!(r
            .register(Arc::new(Odd("mod:backwards", FormatRange::new(3, 2))))
            .is_err());
        r.register(Arc::new(Odd("mod:ok", FormatRange::new(1, 2))))
            .unwrap();
        assert!(
            r.register(Arc::new(Odd("mod:ok", FormatRange::new(1, 2))))
                .is_err(),
            "two adapters for one namespace"
        );
    }

    #[test]
    fn a_realm_without_the_module_gets_nothing_and_the_canonical_payload_is_retained_for_a_realm_that_has_it(
    ) {
        let reg = registry(FakeAdapter::new(2, 3));
        let mut canonical: BTreeMap<String, Extension> = BTreeMap::new();
        canonical.insert("mod:fake".into(), payload(2, b"fake-state"));

        // a realm without the module: the profile does not list it, so the payload is deferred and not one byte is written
        let mut without = realm(false);
        let profile = profile_with(reg.supported_on(&mut without));
        assert!(profile.extensions.is_empty());
        let out = reg.evaluate(&canonical, &profile);
        assert_eq!(
            out[0].disposition,
            ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm)
        );
        let applied = reg.apply_all(&mut without, &canonical, &profile, &HashMap::new());
        assert_eq!(
            applied[0].disposition,
            ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm)
        );
        assert_eq!(without.writes, 0);
        assert!(without.data.is_empty(), "the destination receives nothing");
        assert_eq!(
            canonical["mod:fake"].payload.0, b"fake-state",
            "the canonical payload is untouched"
        );

        // a later realm that has it receives it
        let mut with = realm(true);
        let profile = profile_with(reg.supported_on(&mut with));
        assert_eq!(profile.extensions.len(), 1);
        let applied = reg.apply_all(&mut with, &canonical, &profile, &HashMap::new());
        assert_eq!(applied[0].disposition, ExtensionDisposition::Applied);
        assert_eq!(with.data[&7], b"fake-state");
        // and applying again does not write again
        let state: HashMap<String, String> = [(
            "mod:fake".to_string(),
            canonical["mod:fake"].content_hash.clone(),
        )]
        .into_iter()
        .collect();
        assert_eq!(
            reg.apply_all(&mut with, &canonical, &profile, &state)[0].disposition,
            ExtensionDisposition::AlreadyApplied
        );
        assert_eq!(with.writes, 1);
        // a changed payload is applied again
        canonical.insert("mod:fake".into(), payload(2, b"newer-state"));
        assert_eq!(
            reg.apply_all(&mut with, &canonical, &profile, &state)[0].disposition,
            ExtensionDisposition::Applied
        );
        assert_eq!(with.data[&7], b"newer-state");
    }

    #[test]
    fn a_payload_in_a_format_the_realm_does_not_understand_is_deferred_explicitly_older_or_newer() {
        let reg = registry(FakeAdapter::new(2, 3));
        let mut with = realm(true);
        let profile = profile_with(reg.supported_on(&mut with));
        for (format, expected) in [
            (
                1,
                ExtensionCompatibility::FormatTooOld {
                    payload: 1,
                    realm: FormatRange::new(2, 3),
                },
            ),
            (
                4,
                ExtensionCompatibility::FormatTooNew {
                    payload: 4,
                    realm: FormatRange::new(2, 3),
                },
            ),
        ] {
            let mut canonical = BTreeMap::new();
            canonical.insert("mod:fake".to_string(), payload(format, b"x"));
            assert_eq!(
                reg.evaluate(&canonical, &profile)[0].disposition,
                ExtensionDisposition::Deferred(expected.clone())
            );
            let applied = reg.apply_all(&mut with, &canonical, &profile, &HashMap::new());
            assert_eq!(
                applied[0].disposition,
                ExtensionDisposition::Deferred(expected)
            );
            assert!(
                with.data.is_empty(),
                "nothing is written for an incompatible format"
            );
            assert_eq!(canonical["mod:fake"].payload.0, b"x");
        }
        let mut ok = BTreeMap::new();
        ok.insert("mod:fake".to_string(), payload(3, b"x"));
        assert_eq!(
            reg.evaluate(&ok, &profile)[0].disposition,
            ExtensionDisposition::Applied
        );
    }

    #[test]
    fn a_namespace_nobody_serves_and_the_managers_own_blobs_are_never_applied() {
        let reg = registry(FakeAdapter::new(1, 1));
        let mut with = realm(true);
        let profile = profile_with(reg.supported_on(&mut with));
        let mut canonical = BTreeMap::new();
        canonical.insert("mod:somebody-elses".to_string(), payload(1, b"opaque"));
        canonical.insert("coa:unlisted-settings".to_string(), payload(1, b"{}"));
        let out = reg.apply_all(&mut with, &canonical, &profile, &HashMap::new());
        assert_eq!(
            out.iter()
                .find(|o| o.namespace == "mod:somebody-elses")
                .unwrap()
                .disposition,
            ExtensionDisposition::Deferred(ExtensionCompatibility::MissingOnRealm)
        );
        assert_eq!(
            out.iter()
                .find(|o| o.namespace == "coa:unlisted-settings")
                .unwrap()
                .disposition,
            ExtensionDisposition::CanonicalOnly
        );
        assert_eq!(with.writes, 0);
    }

    #[test]
    fn an_adapter_that_fails_or_is_handed_rubbish_changes_nothing_and_claims_nothing() {
        let adapter = FakeAdapter::new(2, 3);
        let reg = registry(adapter.clone());
        let mut with = realm(true);
        let profile = profile_with(reg.supported_on(&mut with));
        let mut canonical = BTreeMap::new();
        canonical.insert("mod:fake".to_string(), payload(2, b"state"));
        *adapter.fail_apply.lock().unwrap() = true;
        let out = reg.apply_all(&mut with, &canonical, &profile, &HashMap::new());
        assert!(
            matches!(out[0].disposition, ExtensionDisposition::Failed(_)),
            "{:?}",
            out[0]
        );
        *adapter.fail_apply.lock().unwrap() = false;

        // a payload that fails the adapter's own validation (too large) and one whose content does not match its hash
        canonical.insert("mod:fake".into(), payload(2, &[7u8; 65]));
        assert!(matches!(
            reg.apply_all(&mut with, &canonical, &profile, &HashMap::new())[0].disposition,
            ExtensionDisposition::Failed(_)
        ));
        let mut corrupt = payload(2, b"state");
        corrupt.payload.0[0] ^= 1;
        canonical.insert("mod:fake".into(), corrupt);
        assert!(matches!(
            reg.apply_all(&mut with, &canonical, &profile, &HashMap::new())[0].disposition,
            ExtensionDisposition::Failed(_)
        ));
        assert_eq!(with.writes, 0);
    }

    #[test]
    fn exports_come_only_from_adapters_whose_module_is_present_and_are_validated() {
        let reg = registry(FakeAdapter::new(2, 3));
        let mut without = realm(false);
        without.data.insert(7, b"stray".to_vec());
        assert!(
            reg.export_all(&mut without).0.is_empty(),
            "no module, no export"
        );
        let mut with = realm(true);
        assert!(
            reg.export_all(&mut with).0.is_empty(),
            "a character without module data exports nothing"
        );
        with.data.insert(7, b"state".to_vec());
        let (out, problems) = reg.export_all(&mut with);
        assert!(problems.is_empty());
        assert_eq!(
            (
                out["mod:fake"].payload.0.as_slice(),
                out["mod:fake"].format_version
            ),
            (b"state".as_slice(), 3)
        );
        with.data.insert(7, vec![1u8; 100]);
        let (out, problems) = reg.export_all(&mut with);
        assert!(
            out.is_empty() && problems.len() == 1,
            "a payload the adapter itself rejects contributes nothing: {problems:?}"
        );
    }
}
