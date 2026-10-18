mod challenge;
mod proposal;
mod reset;
mod transaction;

#[cfg(test)]
pub(in crate::domain::mobile_relay) use challenge::read_kt_authority_challenge;
#[cfg(test)]
pub(in crate::domain::mobile_relay) use proposal::{
    authority_configuration_matches, parse_kt_authority_proposal,
};
pub(in crate::domain::mobile_relay) use transaction::key_transparency_configure_authority;
