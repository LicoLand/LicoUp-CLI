use super::super::*;

pub(super) fn test_store(name: &str) -> ClientStateStore {
    let dir: PathBuf = env::temp_dir().join(format!("lico-pairing-skill-{}-{}", name, timestamp()));
    fs::create_dir_all(&dir).unwrap();
    ClientStateStore::new(dir).unwrap()
}

pub(super) fn temp_test_dir(name: &str) -> PathBuf {
    let dir = env::temp_dir().join(format!(
        "lico-skill-hub-{}-{}-{}",
        name,
        timestamp(),
        timestamp()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir.canonicalize().unwrap()
}
