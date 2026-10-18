use super::support::*;

#[cfg(target_os = "macos")]
use flate2::{Compression, write::GzEncoder};
#[cfg(target_os = "macos")]
use std::io;
#[cfg(target_os = "macos")]
use tar::Builder;

/// An unsigned candidate must be rejected before the native replacement
/// script can mutate the installed app.
#[cfg(target_os = "macos")]
#[test]
fn client_update_native_runner_rejects_unsigned_macos_candidate_before_replacement() {
    let fixture = UpdateFixture::new();
    let archive = fixture.root.join("client-update.tar.gz");
    write_app_archive(&archive, b"staged");
    let artifact = app_bundle_artifact(&archive);
    let manifest =
        fixture.sign_manifest(fixture.unsigned_manifest(json!([release("999.0.0", artifact,)])));
    let mut params = fixture.checked_params(manifest);
    params["sourcePath"] = json!(archive);
    download(&params).unwrap();
    let _ = verify_staged_selection(&params).unwrap();

    let install_root = fixture.root.join("Applications");
    let current_app = install_root.join("LicoUp.app");
    fs::create_dir_all(current_app.join("Contents")).unwrap();
    fs::write(current_app.join("Contents/Info.plist"), b"current").unwrap();
    params["dataRoot"] = json!(fixture.root.join("data"));
    params["installRoot"] = json!(install_root);
    params["guiPid"] = json!("4242");
    params["waitForScript"] = json!(true);
    params["execute"] = json!(true);

    let error = super::super::apply::apply(&params).unwrap_err().to_string();
    assert!(
        error.contains("signature") || error.contains("authenticity"),
        "unexpected rejection: {error}"
    );
    assert_eq!(
        fs::read(current_app.join("Contents/Info.plist")).unwrap(),
        b"current"
    );
    assert!(!fixture.staging.join(".scripts").exists());
}

#[test]
fn client_update_macos_authenticity_requires_same_developer_id_and_platform_trust() {
    use super::super::native_runner::macos_integrity::{
        CommandEvidence, verify_macos_update_authenticity_with_runner,
    };

    let fixture = UpdateFixture::new();
    let current = fixture.root.join("installed/LicoUp.app");
    let staged = fixture.root.join("staged/LicoUp.app");
    let mut calls = Vec::new();
    verify_macos_update_authenticity_with_runner(
        &current,
        &staged,
        "land.lico.licoup",
        |program, args| {
            calls.push((program.to_string(), args.to_vec()));
            let output = if args.iter().any(|arg| arg == "--requirements") {
                "designated => anchor apple generic and identifier \"land.lico.licoup\" and certificate leaf[subject.OU] = TEAM123456\n"
            } else if args.iter().any(|arg| arg == "--verbose=4") {
                "Identifier=land.lico.licoup\nTeamIdentifier=TEAM123456\nAuthority=Developer ID Application: Synthetic (TEAM123456)\nflags=0x10000(runtime)\nTimestamp=Aug 11, 2026 at 12:00:00\n"
            } else {
                ""
            };
            Ok(CommandEvidence {
                success: true,
                stdout: String::new(),
                stderr: output.to_string(),
            })
        },
    )
    .unwrap();

    assert!(
        calls
            .iter()
            .any(|(program, args)| program == "/usr/bin/codesign"
                && args.iter().any(|arg| arg.starts_with("-R=")))
    );
    assert!(
        calls
            .iter()
            .any(|(program, args)| program == "/usr/bin/xcrun"
                && args.starts_with(&["stapler".to_string(), "validate".to_string()]))
    );
    assert!(
        calls
            .iter()
            .any(|(program, args)| program == "/usr/sbin/spctl"
                && args.iter().any(|arg| arg == "execute"))
    );

    let rejected = verify_macos_update_authenticity_with_runner(
        &current,
        &staged,
        "land.lico.licoup",
        |_program, args| {
            let output = if args.iter().any(|arg| arg == "--requirements") {
                "designated => anchor apple generic and identifier \"land.lico.licoup\"\n"
            } else if args.iter().any(|arg| arg == "--verbose=4") {
                "Identifier=land.lico.licoup\nTeamIdentifier=TEAM123456\nAuthority=Apple Development: Synthetic\nflags=0x10000(runtime)\nTimestamp=Aug 11, 2026 at 12:00:00\n"
            } else {
                ""
            };
            Ok(CommandEvidence {
                success: true,
                stdout: String::new(),
                stderr: output.to_string(),
            })
        },
    )
    .unwrap_err();
    assert!(rejected.to_string().contains("Developer ID Application"));
}

/// Script generation is validated on every platform: the templates must use
/// only the allowlisted OS-bundled commands and never the banned tools, and
/// injected argv values are validated before they reach a script.
#[test]
fn client_update_native_runner_scripts_use_only_bundled_tools() {
    use super::super::native_runner::script::{ScriptAction, platform_script_for_test};
    for (platform, action) in [
        ("macos", ScriptAction::Apply),
        ("linux", ScriptAction::Apply),
    ] {
        let script = platform_script_for_test(platform, action);
        for banned in [
            "curl", "wget", "gh ", "python", "node", "unzip", "tar ", "pkill", "pgrep", "shutil",
            "rm -rf /",
        ] {
            assert!(
                !script.contains(banned),
                "{platform} script must not use banned tool '{banned}'"
            );
        }
        assert!(script.contains("#!/bin/sh"));
        assert!(script.contains("kill -0"));
        assert!(script.contains("--"));
    }
    let windows_apply = platform_script_for_test("windows", ScriptAction::Apply);
    for script in [windows_apply] {
        for banned in [
            "curl",
            "wget",
            "Invoke-WebRequest",
            "python",
            "node",
            "Expand-Archive",
        ] {
            assert!(
                !script.contains(banned),
                "windows script must not use '{banned}'"
            );
        }
        for required in [
            "Get-Process",
            "Start-Sleep",
            "Remove-Item",
            "Start-Process",
            "-PassThru",
            "$handoff.rejected",
            "'claimed'",
        ] {
            assert!(
                script.contains(required),
                "windows script must use '{required}'"
            );
        }
    }
    assert!(windows_apply.contains("Test-Path"));
    let macos_apply = platform_script_for_test("macos", ScriptAction::Apply);
    for required in [
        "/usr/bin/codesign --verify --deep --strict",
        "/usr/bin/xcrun stapler validate",
        "/usr/sbin/spctl --assess --type execute",
        "restore_pre_claim",
        "BACKUP=\"$6\"",
        "HANDOFF=\"$7\"",
        "REJECTED=\"$HANDOFF.rejected\"",
        "/usr/bin/open -W",
        "'\"state\":\"claimed\"'",
    ] {
        assert!(macos_apply.contains(required));
    }
    assert!(
        macos_apply.find("codesign --verify").unwrap() < macos_apply.find("/bin/rm -rf").unwrap()
    );
}

#[test]
fn client_update_native_runner_rejects_injected_argv_values() {
    use super::super::native_runner::script::validate_script_paths;
    for value in [
        "/fixture-root/evil;rm -rf /",
        "/fixture-root/$(id)",
        "/fixture-root/quote'",
        "/fixture-root/quote\"",
        "/fixture-root/backtick`",
        "/fixture-root/pipe|",
        "/fixture-root/amp&",
        "relative-path",
        "",
    ] {
        assert!(
            validate_script_paths(&[value]).is_err(),
            "argv value must be rejected: {value:?}"
        );
    }
    // Absolute paths with spaces and drive letters are accepted.
    let separator = char::from(92);
    let windows_fixture = format!("C:{separator}fixture-root{separator}Lico{separator}App");
    assert!(validate_script_paths(&["/fixture-root/LicoUp data/App", &windows_fixture]).is_ok());
}

#[test]
fn client_update_native_runner_target_guard_rejects_filesystem_roots() {
    use super::super::native_runner::plan::ensure_guarded_target_for_test;
    for target in [Path::new("/"), Path::new("C:\\"), Path::new("/tmp")] {
        assert!(
            ensure_guarded_target_for_test(target).is_err(),
            "target must be rejected: {target:?}"
        );
    }
    assert!(ensure_guarded_target_for_test(Path::new("/fixture-root/LicoUp")).is_ok());
}

#[cfg(target_os = "macos")]
fn write_app_archive(path: &Path, marker: &[u8]) {
    let encoder = GzEncoder::new(fs::File::create(path).unwrap(), Compression::default());
    let mut archive = Builder::new(encoder);
    let mut directory = tar::Header::new_gnu();
    directory.set_entry_type(tar::EntryType::Directory);
    directory.set_mode(0o755);
    directory.set_size(0);
    directory.set_cksum();
    archive
        .append_data(&mut directory, "LicoUp.app/Contents", io::empty())
        .unwrap();
    let mut file = tar::Header::new_gnu();
    file.set_mode(0o644);
    file.set_size(marker.len() as u64);
    file.set_cksum();
    archive
        .append_data(&mut file, "LicoUp.app/Contents/Info.plist", marker)
        .unwrap();
    archive.into_inner().unwrap().finish().unwrap();
}

#[cfg(target_os = "macos")]
fn app_bundle_artifact(path: &Path) -> Value {
    json!({
        "targetId": TARGET_ID,
        "platform": "macos",
        "osFamily": "darwin",
        "arch": "arm64",
        "installerStrategy": "app-bundle-replacement",
        "url": url::Url::from_file_path(path).unwrap().to_string(),
        "fileName": "client-update.tar.gz",
        "size": fs::metadata(path).unwrap().len(),
        "sha256": sha256_hex(&fs::read(path).unwrap()),
        "applicationName": "LicoUp.app",
        "bundleId": "land.lico.licoup",
    })
}
