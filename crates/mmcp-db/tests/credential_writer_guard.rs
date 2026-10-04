#![allow(clippy::unwrap_used, clippy::expect_used)]
//! Mechanical guard: every function that writes a credential is classified, and a classified
//! incrementing writer really calls the epoch owner.
//!
//! A new credential writer fails this test until it is added to the table below with its class.
//! That keeps a future credential path from silently skipping the epoch increment.

use std::fs;
use std::path::{Path, PathBuf};

/// What a credential-table write does to the credential epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    /// A credential change: it must call `bump_credential_epoch`.
    Bumps,
    /// A write at account creation: no other session can hold the account, so it must not bump.
    Initial,
    /// A write that changes no credential: it must not bump.
    NonCredential,
}

/// Repository files that own credential tables, with their classified writer functions.
const CLASSIFIED_WRITERS: &[(&str, &[(&str, Class)])] = &[
    (
        "passkey_repo.rs",
        &[
            ("create", Class::Bumps),
            ("delete", Class::Bumps),
            ("update_after_auth", Class::NonCredential),
        ],
    ),
    (
        "oauth_repo.rs",
        &[
            ("insert", Class::Initial),
            ("update_tokens", Class::NonCredential),
        ],
    ),
    (
        "user_repo.rs",
        &[("insert", Class::Initial), ("update_profile", Class::Bumps)],
    ),
];

/// Repository files that may touch a credential table without being classified writers.
const CREDENTIAL_TABLE_OWNERS: &[&str] = &[
    "passkey_repo.rs",
    "oauth_repo.rs",
    "user_repo.rs",
    "credential_epoch.rs",
];

const CREDENTIAL_TABLE_MARKERS: &[&str] = &[
    "entities::user",
    "entities::passkey_credential",
    "entities::oauth_account",
    "password_hash",
];

const WRITE_MARKERS: &[&str] = &[
    ".insert(",
    ".update(",
    "update_many",
    "delete_many",
    "delete_by_id",
    "insert_many",
];

fn repository_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src/repository")
}

fn read(file_name: &str) -> String {
    fs::read_to_string(repository_dir().join(file_name)).expect("read repository source")
}

/// Splits a source file into `(function name, text)` pairs at every top-level `fn`.
fn top_level_functions(source: &str) -> Vec<(String, String)> {
    let mut functions: Vec<(String, String)> = Vec::new();
    for line in source.lines() {
        let declaration = ["pub async fn ", "pub(crate) async fn ", "async fn ", "fn "]
            .iter()
            .find_map(|prefix| line.strip_prefix(prefix));
        if let Some(rest) = declaration {
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            functions.push((name, String::new()));
        }
        if let Some((_, text)) = functions.last_mut() {
            text.push_str(line);
            text.push('\n');
        }
    }
    functions
}

fn writes(text: &str) -> bool {
    WRITE_MARKERS.iter().any(|marker| text.contains(marker))
}

fn bumps(text: &str) -> bool {
    text.contains("bump_credential_epoch(")
}

#[test]
fn every_credential_writer_is_classified_and_honours_its_class() {
    for (file_name, classified) in CLASSIFIED_WRITERS {
        for (name, text) in top_level_functions(&read(file_name)) {
            let class = classified
                .iter()
                .find(|(classified_name, _)| *classified_name == name)
                .map(|(_, class)| *class);
            match class {
                None => assert!(
                    !writes(&text),
                    "{file_name}::{name} writes but is not classified in the guard table"
                ),
                Some(Class::Bumps) => assert!(
                    bumps(&text),
                    "{file_name}::{name} changes a credential but never calls bump_credential_epoch"
                ),
                Some(Class::Initial | Class::NonCredential) => assert!(
                    !bumps(&text),
                    "{file_name}::{name} is classified {class:?} but calls bump_credential_epoch"
                ),
            }
        }
    }
}

#[test]
fn every_classified_writer_still_exists() {
    for (file_name, classified) in CLASSIFIED_WRITERS {
        let names: Vec<String> = top_level_functions(&read(file_name))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        for (classified_name, _) in *classified {
            assert!(
                names.iter().any(|name| name == classified_name),
                "{file_name}::{classified_name} is in the guard table but no longer exists"
            );
        }
    }
}

#[test]
fn no_other_repository_touches_a_credential_table() {
    for entry in fs::read_dir(repository_dir()).expect("list repository sources") {
        let file_name = entry
            .expect("directory entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        if CREDENTIAL_TABLE_OWNERS.contains(&file_name.as_str()) {
            continue;
        }
        let source = read(&file_name);
        for marker in CREDENTIAL_TABLE_MARKERS {
            assert!(
                !source.contains(marker),
                "{file_name} mentions `{marker}`: a credential writer outside the owners skips the epoch"
            );
        }
    }
}
