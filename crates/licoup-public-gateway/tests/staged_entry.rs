//! The committed staged entry is a release-stage placeholder, never a silently
//! shipped program.
//!
//! `crates/licoup-mcp/package` is packaged exactly as committed, so a checkout
//! builds a deterministic payload and the payload contract can be tested without
//! a compiler. The native entry the manifest declares is filled by the release
//! stage (`tools/distribution/client-release-package-stage.mjs`) before a real
//! build, and the committed bytes therefore have to stay recognisable as that
//! placeholder: an executable, non-script text file that names the build it
//! stands in for, with no compiled-program header.
//!
//! This is the artifact half of the optional composition. The client ships no
//! copy of this payload at all, so a program appearing here without the release
//! stage having run would be exactly the mandatory payload the composition
//! removes — and the release stage refuses an entry that is not a program for
//! the target platform, which is why both halves have to agree on these bytes.
//!
//! Nothing here executes the entry, resolves a path outside the committed
//! package or reaches the network.

use std::fs;
use std::path::{Path, PathBuf};

const NATIVE_ENTRY: &str = "bin/lico-subagent-mcp";
const BINARY_TARGET: &str = "lico-subagent-mcp";

/// The interpreter extensions the payload contract refuses on an entry name.
const INTERPRETER_EXTENSIONS: [&str; 24] = [
    ".py", ".pyw", ".rb", ".pl", ".php", ".lua", ".sh", ".bash", ".zsh", ".ksh", ".fish", ".ps1",
    ".bat", ".cmd", ".js", ".mjs", ".cjs", ".ts", ".jar", ".class", ".exe.bat", ".vbs", ".wsf",
    ".awk",
];

/// The headers a compiled program carries. The placeholder must carry none.
const COMPILED_HEADERS: [(&str, &[u8]); 4] = [
    ("Mach-O 64-bit", &[0xcf, 0xfa, 0xed, 0xfe]),
    ("Mach-O universal", &[0xca, 0xfe, 0xba, 0xbe]),
    ("ELF", &[0x7f, 0x45, 0x4c, 0x46]),
    ("PE", &[0x4d, 0x5a]),
];

fn starts_with(bytes: &[u8], header: &[u8]) -> bool {
    bytes.len() >= header.len() && bytes[..header.len()] == *header
}

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("package")
}

fn entry_path() -> PathBuf {
    package_root().join(NATIVE_ENTRY)
}

fn crate_manifest() -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the crate manifest is readable")
}

#[test]
fn the_committed_entry_is_the_release_stage_placeholder_and_not_a_program() {
    let path = entry_path();
    let metadata = fs::symlink_metadata(&path).expect("the declared entry is committed");
    assert!(
        metadata.file_type().is_file() && !metadata.file_type().is_symlink(),
        "the declared entry must be a regular committed file"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_ne!(
            metadata.permissions().mode() & 0o111,
            0,
            "the declared entry must be executable, as the payload contract requires"
        );
    }

    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .expect("the entry has a name");
    for extension in INTERPRETER_EXTENSIONS {
        assert!(
            !name.to_ascii_lowercase().ends_with(extension),
            "an interpreter script name is refused by the payload contract: {name}"
        );
    }

    let bytes = fs::read(&path).expect("the declared entry is readable");
    assert!(!bytes.is_empty(), "the declared entry must not be empty");
    assert!(
        !starts_with(&bytes, b"#!"),
        "the payload contract refuses an entry that carries a shebang"
    );
    for (kind, header) in COMPILED_HEADERS {
        assert!(
            !starts_with(&bytes, header),
            "a {kind} program is committed in the staged entry; only the release stage may put a compiled program here"
        );
    }

    let text = std::str::from_utf8(&bytes).expect("the placeholder is text");
    // The placeholder has to say which build it stands in for: that is the
    // command the release stage runs, and it has to name the target the crate
    // actually builds.
    assert!(
        text.contains(&format!("cargo build --bin {BINARY_TARGET}")),
        "the placeholder must name the build that fills it"
    );
    assert!(
        text.contains("tools/scripts/client-release-package-index.mjs"),
        "the placeholder must name the packaging step it precedes"
    );
    assert!(
        text.contains("staged entry"),
        "the placeholder must describe itself as the staged entry"
    );
}

#[test]
fn the_staged_entry_name_is_a_binary_target_this_crate_builds() {
    let manifest = crate_manifest();
    let declared = manifest
        .lines()
        .skip_while(|line| line.trim() != "[[bin]]")
        // `skip_while` yields the line it stopped on; the section body is what
        // follows it.
        .skip(1)
        .take_while(|line| !line.trim().starts_with('['))
        .find_map(|line| line.trim().strip_prefix("name = "))
        .map(|value| value.trim().trim_matches('"').to_owned())
        .expect("the crate declares a binary target");
    assert_eq!(
        declared, BINARY_TARGET,
        "the release stage builds the binary the declared entry names"
    );
    assert_eq!(
        entry_path()
            .file_name()
            .and_then(|value| value.to_str())
            .expect("the entry has a name"),
        declared,
        "the staged entry file is the binary target itself, not a copy under another name"
    );
}

/// The named staging tool is the one that fills this entry, and it refuses a
/// placeholder. Named here so the artifact and the release step cannot drift
/// apart silently.
#[test]
fn the_release_stage_that_fills_this_entry_refuses_a_placeholder() {
    let tool = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tools/distribution/client-release-package-stage.mjs"),
    )
    .expect("the release staging tool is committed");
    assert!(
        tool.contains("package_stage_entry_not_compiled"),
        "the release stage must refuse the placeholder this entry is"
    );
    assert!(
        tool.contains("chmodSync(entry.stagedPath, 0o755)"),
        "the release stage must publish the entry executable"
    );
    assert!(
        tool.contains("buildNativeSidecars"),
        "the release stage must build through the client's own native build owner"
    );
}
