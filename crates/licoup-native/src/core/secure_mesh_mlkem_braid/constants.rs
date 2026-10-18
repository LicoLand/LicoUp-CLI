use libcrux_ml_kem::mlkem1024::incremental::{self, Ciphertext1, Ciphertext2};

pub const ML_KEM_BRAID_CHUNK_BYTES: usize = 32;
pub const ML_KEM_BRAID_HEADER_BYTES: usize = 64;
pub const ML_KEM_BRAID_EK_BYTES: usize = 1_536;
pub const ML_KEM_BRAID_CT1_BYTES: usize = 1_408;
pub const ML_KEM_BRAID_CT2_BYTES: usize = 160;
pub const ML_KEM_BRAID_MAC_BYTES: usize = 32;
pub const ML_KEM_BRAID_TRANSITION_COUNT: usize = 13;

const _: [(); ML_KEM_BRAID_HEADER_BYTES] = [(); incremental::pk1_len()];
const _: [(); ML_KEM_BRAID_EK_BYTES] = [(); incremental::pk2_len()];
const _: [(); ML_KEM_BRAID_CT1_BYTES] = [(); Ciphertext1::len()];
const _: [(); ML_KEM_BRAID_CT2_BYTES] = [(); Ciphertext2::len()];

pub(super) const PROTOCOL_INFO: &[u8] = b"LicoMesh_MLKEM1024_HMAC-SHA256";
pub(super) const AUTH_UPDATE_LABEL: &[u8] = b":Authenticator Update";
pub(super) const OUTPUT_KEY_LABEL: &[u8] = b":SCKA Key";
pub(super) const HEADER_MAC_LABEL: &[u8] = b":ekheader";
pub(super) const CIPHERTEXT_MAC_LABEL: &[u8] = b":ciphertext";
pub(super) const INITIAL_EPOCH: u64 = 1;
pub(super) const MAX_SOURCE_CHUNKS: usize = ML_KEM_BRAID_EK_BYTES / ML_KEM_BRAID_CHUNK_BYTES;
pub(super) const GF_REDUCTION_POLYNOMIAL: u32 = 0x100b;
pub(super) const PERSISTENCE_REVISION: u8 = 2;
pub(super) const MAX_PERSISTED_SESSION_BYTES: usize = 512 * 1024;
pub(super) const ENCODED_CHUNK_BYTES: usize = ((ML_KEM_BRAID_CHUNK_BYTES + 2) * 8 + 5) / 6;
