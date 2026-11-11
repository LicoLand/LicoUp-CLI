use std::path::Path;

use super::super::command::GatewayRunner;
use crate::platform::local_service::process::SpawnFailure;

struct MissingRunner;

impl GatewayRunner for MissingRunner {
    fn spawn(
        &self,
        _executable: &str,
        _port: u16,
        _runtime_dir: &Path,
        _config_path: &Path,
    ) -> Result<u32, SpawnFailure> {
        Err(SpawnFailure::Missing)
    }
}

#[test]
fn runner_contract_uses_typed_static_spawn_failures() {
    let failure = MissingRunner
        .spawn("missing", 24189, Path::new("runtime"), Path::new("config"))
        .unwrap_err();
    assert_eq!(failure, SpawnFailure::Missing);
}
