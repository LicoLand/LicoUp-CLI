use super::super::clock::{
    current_secure_mesh_kt_gate_epoch_seconds, set_kt_freshness_now_override,
};

#[test]
fn freshness_clock_override_is_scoped_and_nested() {
    let outer = set_kt_freshness_now_override(11);
    assert_eq!(current_secure_mesh_kt_gate_epoch_seconds().unwrap(), 11);
    {
        let _inner = set_kt_freshness_now_override(22);
        assert_eq!(current_secure_mesh_kt_gate_epoch_seconds().unwrap(), 22);
    }
    assert_eq!(current_secure_mesh_kt_gate_epoch_seconds().unwrap(), 11);
    drop(outer);
}
