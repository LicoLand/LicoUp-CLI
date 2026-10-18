#[cfg(any(test, target_os = "android"))]
pub mod android_ffi;
pub mod commands;
pub mod generated;
#[cfg(any(test, target_os = "ios"))]
pub mod ios_ffi;
pub mod secure_mesh_mobile_ffi;
