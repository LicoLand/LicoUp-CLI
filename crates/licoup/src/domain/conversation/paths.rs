use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

pub(crate) fn home_dir() -> PathBuf {
    home_dir_from_env(|name| env::var_os(name))
}

pub(crate) fn home_dir_from_env<F>(var: F) -> PathBuf
where
    F: Fn(&str) -> Option<OsString>,
{
    licoup_foundation::platform::paths::env_home_from(var).unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn expand_home(value: &str) -> PathBuf {
    expand_home_from(value, home_dir)
}

pub(crate) fn expand_home_from<F>(value: &str, home: F) -> PathBuf
where
    F: Fn() -> PathBuf,
{
    if value == "~" {
        return home();
    }
    if let Some(rest) = value.strip_prefix("~/") {
        return home().join(rest);
    }
    if let Some(rest) = value.strip_prefix("~\\") {
        return home().join(rest);
    }
    PathBuf::from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_cross_platform_home_fallbacks_and_tilde_paths() {
        let separator = char::from(92).to_string();
        let home_path = ["", "Profile", "Arc"].join(&separator);
        let home = home_dir_from_env(|name| match name {
            "HOMEDRIVE" => Some(OsString::from("C:")),
            "HOMEPATH" => Some(OsString::from(&home_path)),
            _ => None,
        });
        assert_eq!(
            home,
            PathBuf::from(["C:", "Profile", "Arc"].join(&separator))
        );
        assert_eq!(
            expand_home_from(&["~", ".codex", "sessions"].join(&separator), || home
                .clone()),
            home.join([".codex", "sessions"].join(&separator))
        );
    }
}
