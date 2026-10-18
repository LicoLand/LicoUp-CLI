use super::*;

fn windows_path(parts: &[&str]) -> String {
    parts.join(&char::from(92).to_string())
}

#[test]
fn archive_home_uses_windows_userprofile_when_home_is_missing() {
    let profile = windows_path(&["C:", "Profile", "LicoMesh"]);
    let resolved = home_dir_from_env(|name| match name {
        "USERPROFILE" => Some(OsString::from(&profile)),
        _ => None,
    });

    assert_eq!(resolved, PathBuf::from(profile));
}

#[test]
fn archive_home_uses_windows_drive_and_homepath_fallback() {
    let home_path = windows_path(&["", "Profile", "LicoMesh"]);
    let resolved = home_dir_from_env(|name| match name {
        "HOMEDRIVE" => Some(OsString::from("C:")),
        "HOMEPATH" => Some(OsString::from(&home_path)),
        _ => None,
    });

    assert_eq!(
        resolved,
        PathBuf::from(windows_path(&["C:", "Profile", "LicoMesh"]))
    );
}

#[test]
fn archive_expand_home_accepts_windows_style_tilde_paths() {
    let profile = PathBuf::from(windows_path(&["C:", "Profile", "LicoMesh"]));
    let sessions = windows_path(&[".codex", "sessions"]);
    let expanded = expand_home_from(&windows_path(&["~", ".codex", "sessions"]), || {
        profile.clone()
    });

    assert_eq!(expanded, profile.join(sessions));
}
