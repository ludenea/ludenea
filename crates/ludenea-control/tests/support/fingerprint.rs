//! The wire fingerprint of a protocol, shared by the `typescript` tests of the control and
//! the project protocols so both pin the same kind of hash.

use std::any::TypeId;
use std::collections::BTreeSet;
use std::fs;

use ludenea_control::Hash64;
use ts_rs::{Config, TS, TypeVisitor};
use xxhash_rust::xxh3::xxh3_64;

const CHANGELOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../CHANGELOG.md");

/// Asserts that the wire shape of `protocol` (such as "Control protocol"), whose messages are
/// `Client` and `Server`, is the one pinned for `version` and that CHANGELOG.md names it.
/// `constants` prefixes the names of the version and fingerprint constants to update.
pub fn assert_wire_shape_is_pinned<Client: TS + 'static, Server: TS + 'static>(
    protocol: &str,
    version: u32,
    pinned: Hash64,
    constants: &str,
) {
    let fingerprint = wire_fingerprint::<Client, Server>(&Config::from_env());
    let lowercase = protocol.to_lowercase();
    assert!(
        fingerprint == pinned.0,
        "the wire shape of {lowercase} {version} changed (fingerprint {fingerprint:016x}, \
         pinned {:016x}): set {constants}_FINGERPRINT to it, bump {constants}_VERSION unless \
         this version is unreleased, and record the change in CHANGELOG.md",
        pinned.0
    );
    let entry = format!("{protocol} {version}, fingerprint `{fingerprint:016x}`");
    let changelog = fs::read_to_string(CHANGELOG)
        .unwrap_or_else(|error| panic!("{CHANGELOG} cannot be read: {error}"));
    assert!(
        changelog.contains(&entry),
        "{CHANGELOG} does not name this wire shape: write {entry} in the entry of {lowercase} \
         {version}"
    );
}

/// xxh3 of the TypeScript declarations of every type the messages reach, wherever it is
/// defined, in sorted order and without their documentation, so only a change to what travels
/// moves it.
fn wire_fingerprint<Client: TS + 'static, Server: TS + 'static>(config: &Config) -> u64 {
    let mut declarations = Declarations {
        config,
        seen: BTreeSet::new(),
        texts: Vec::new(),
    };
    declarations.visit::<Client>();
    declarations.visit::<Server>();
    declarations.texts.sort();
    xxh3_64(declarations.texts.join("\n").as_bytes())
}

/// Walks the types `export_all` writes, collecting their declarations.
struct Declarations<'a> {
    config: &'a Config,
    seen: BTreeSet<TypeId>,
    texts: Vec<String>,
}

impl TypeVisitor for Declarations<'_> {
    fn visit<T: TS + 'static + ?Sized>(&mut self) {
        if T::output_path().is_none() || !self.seen.insert(TypeId::of::<T>()) {
            return;
        }
        self.texts.push(without_docs(&T::decl(self.config)));
        T::visit_dependencies(self);
    }
}

/// `declaration` without the JSDoc blocks ts-rs writes for documented fields and variants,
/// and with its whitespace collapsed, so editing a doc comment leaves it unchanged.
fn without_docs(declaration: &str) -> String {
    let mut rest = declaration;
    let mut text = String::new();
    while let Some(start) = rest.find("/**") {
        text.push_str(&rest[..start]);
        let end = rest[start..]
            .find("*/")
            .unwrap_or_else(|| panic!("an unterminated doc comment in {declaration}"));
        rest = &rest[start + end + 2..];
    }
    text.push_str(rest);
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}
