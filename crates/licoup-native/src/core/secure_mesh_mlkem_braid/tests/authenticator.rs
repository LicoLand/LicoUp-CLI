use super::{
    super::authenticator::RatchetedAuthenticator,
    support::{TEST_SECRET, hex},
};

#[test]
fn authenticator_known_answer() {
    let auth = RatchetedAuthenticator::initialize(1, &TEST_SECRET).unwrap();
    assert_eq!(
        hex(auth.root_key.as_slice()),
        "330afd0d383ea018119058e666e7837cea1ba96ecd7b665db08568d84bdd3608"
    );
    assert_eq!(
        hex(auth.mac_key.as_slice()),
        "ccea858fa08c977ac9fcd792d1daa1fa3bb53c3c72ec9aec6678fca15057bd46"
    );
    let header = (0u8..64).collect::<Vec<_>>();
    assert_eq!(
        hex(&auth.mac_header(1, &header).unwrap()),
        "1b3bd938058873c066f3483672150a5413aa1ed48abbdb65ad02e77d7201046c"
    );
}
