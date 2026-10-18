//! Fail-closed Protocol Line admission at the native composition root.

pub use licoup_protocol_bindings::{
    AUTHORIZATION_REQUIRED, AdmissionRefusal, AuthorityInput, VerifiedProtocolLine,
};

#[cfg(test)]
mod tests {
    use super::{AUTHORIZATION_REQUIRED, AuthorityInput};

    #[test]
    fn native_admission_fails_closed_without_the_fixed_artifact() {
        let refused = AuthorityInput::new(b"")
            .admit()
            .expect_err("an empty input is not the fixed Candidate");
        assert_eq!(refused.code(), AUTHORIZATION_REQUIRED);
    }

    #[test]
    fn native_admission_does_not_invent_a_protocol_line() {
        // Bundle-shaped metadata without the fixed artifact is still refused:
        // only the SDK's verification of the supplied bytes admits a line.
        let candidate = br#"{"artifactVersion":"licoarc.bundle.v1","wireId":"licoarc.protocol-line.v1","generation":1,"lifecycle":"Candidate","definitionStatus":"COMPLETE","sessionEligible":true,"publicationEligible":false,"digestAlgorithm":"sha256","sources":{},"digest":"0000000000000000000000000000000000000000000000000000000000000000"}"#;
        let refused = AuthorityInput::new(candidate)
            .admit()
            .expect_err("a self-declared bundle is not the fixed Candidate");
        assert_eq!(refused.code(), AUTHORIZATION_REQUIRED);
    }
}
