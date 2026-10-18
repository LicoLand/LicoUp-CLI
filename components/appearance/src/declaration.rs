//! What this package declares, and where every declared value comes from.
//!
//! A conversion declaration is a claim about a package's own payload, so a migrated
//! caller reads it instead of recognising a package by name. The two format identities
//! are the one part of the claim this package must not invent: the client's embedded
//! frontier catalogue is the single authority for the source/target pair, and
//! [`licoup_native::domain::client_state_migration::conversion_endpoints`] is that
//! authority's own projection. The declaration here is therefore *derived* rather than
//! restated: when the catalogue moves to the next published pair, this package's
//! committed manifest is stale until it is regenerated, and its own suite says so.
//!
//! [`manifest_json`] is the document the packaging carrier publishes as the package's
//! `manifest.json`, and [`validate`] reads it back through the contract's own parser,
//! so a hand-edited document cannot pass as a declaration the host would accept.

use anyhow::{Context, Result, anyhow};
use licoup_extension_contracts::manifest::{
    ConversionDeclaration, FrozenEndpoints, PackageManifest,
};
use licoup_extension_contracts::wire;
use serde_json::{Value, json};

use crate::{
    CLIENT_COMPATIBILITY, DISPLAY_NAME, ENTRY, HOST_PROTOCOL, PACKAGE_ID, PACKAGE_VERSION,
    PROFILE_ID,
};

/// The published source/target pair the client's own catalogue declares.
///
/// This is the pair every migration caller requires, and the only one a package may be
/// identified for. It is read, never written here: a format name compiled into this
/// package would be a second place to disagree with the conversion the client performs.
pub fn endpoints() -> Result<FrozenEndpoints> {
    let declared = licoup_native::domain::client_state_migration::conversion_endpoints()
        .context("migration_frontier_incomplete")?;
    Ok(FrozenEndpoints::new(
        declared.source_frontier_id,
        declared.target_frontier_id,
    ))
}

/// The conversion this package declares: the pair above, one native entry.
///
/// The declaration is validated through the contract before it is returned, so a
/// package whose own declaration the host would refuse cannot be published from here.
pub fn conversion() -> Result<ConversionDeclaration> {
    let endpoints = endpoints()?;
    let declaration = ConversionDeclaration::new(
        ENTRY,
        [endpoints.source_format()],
        endpoints.target_format(),
    );
    declaration
        .validate()
        .map_err(|failure| anyhow!("{}", failure.code))?;
    Ok(declaration)
}

/// The package's published manifest document.
///
/// The conversion object is the one [`conversion`] returns, so the document and the
/// declaration are one value. The document is read back through
/// [`PackageManifest::from_value`] before it is returned: the contract's parser decides
/// whether this package may be published, not this module's opinion of it.
pub fn manifest_document() -> Result<Value> {
    let declaration = conversion()?;
    let document = json!({
        "schema": wire::MANIFEST,
        "id": PACKAGE_ID,
        "version": PACKAGE_VERSION,
        "displayName": DISPLAY_NAME,
        "hostProtocol": { "major": HOST_PROTOCOL.0, "minimumMinor": HOST_PROTOCOL.1 },
        "compatibility": { "clientVersions": [CLIENT_COMPATIBILITY] },
        "profiles": [{ "id": PROFILE_ID, "major": 1 }],
        "runtime": { "mode": "process", "entry": ENTRY },
        "conversion": serde_json::to_value(&declaration)?,
        "activation": "on-demand",
        "requires": [],
        "optionalRequires": [],
        "permissions": [],
        "contributions": []
    });
    validate(&document)?;
    Ok(document)
}

/// The committed manifest document as text, exactly as the carrier publishes it.
pub fn manifest_json() -> Result<String> {
    let mut document = serde_json::to_string_pretty(&manifest_document()?)?;
    document.push('\n');
    Ok(document)
}

/// Read a manifest document back through the contract's own parser.
pub fn validate(document: &Value) -> Result<PackageManifest> {
    PackageManifest::from_value(document.clone()).map_err(|failure| anyhow!("{}", failure.code))
}
