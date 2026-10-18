use super::*;
use std::path::PathBuf;

mod codec;
mod requests;
mod responses;

fn client() -> AcpImplementation {
    AcpImplementation::new("lico-up", "test").title("LicoUp")
}

fn absolute_test_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    let root = PathBuf::from(format!("C:{}", std::path::MAIN_SEPARATOR));
    #[cfg(not(target_os = "windows"))]
    let root = PathBuf::from(std::path::MAIN_SEPARATOR.to_string());
    root.join("workspace").join("project")
}
