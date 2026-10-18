use std::fmt;

pub const MIN_PADDING_BUCKET_BYTES: usize = 256;
pub const POWER_OF_TWO_PADDING_LIMIT_BYTES: usize = 64 * 1024;
pub const LARGE_PADDING_BUCKET_STEP_BYTES: usize = 64 * 1024;
pub const MAX_PADDING_BUCKET_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthenticatedPaddingBucketError {
    OutsideBounds,
    UnsupportedPowerOfTwo,
    MisalignedLargeBucket,
}

impl fmt::Display for AuthenticatedPaddingBucketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::OutsideBounds => "secure mesh ciphertext bucket is outside bounds",
            Self::UnsupportedPowerOfTwo => {
                "secure mesh ciphertext bucket is not a supported power-of-two bucket"
            }
            Self::MisalignedLargeBucket => {
                "secure mesh ciphertext bucket is not aligned to the large-payload step"
            }
        })
    }
}

impl std::error::Error for AuthenticatedPaddingBucketError {}

pub fn validate_authenticated_padding_bucket(
    ciphertext_size: usize,
) -> Result<(), AuthenticatedPaddingBucketError> {
    if !(MIN_PADDING_BUCKET_BYTES..=MAX_PADDING_BUCKET_BYTES).contains(&ciphertext_size) {
        return Err(AuthenticatedPaddingBucketError::OutsideBounds);
    }
    if ciphertext_size <= POWER_OF_TWO_PADDING_LIMIT_BYTES {
        if !ciphertext_size.is_power_of_two() {
            return Err(AuthenticatedPaddingBucketError::UnsupportedPowerOfTwo);
        }
    } else if ciphertext_size % LARGE_PADDING_BUCKET_STEP_BYTES != 0 {
        return Err(AuthenticatedPaddingBucketError::MisalignedLargeBucket);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        AuthenticatedPaddingBucketError as Error, LARGE_PADDING_BUCKET_STEP_BYTES,
        MAX_PADDING_BUCKET_BYTES, MIN_PADDING_BUCKET_BYTES, POWER_OF_TWO_PADDING_LIMIT_BYTES,
        validate_authenticated_padding_bucket,
    };

    #[test]
    fn authenticated_padding_bucket_accepts_supported_boundaries() {
        for size in [
            MIN_PADDING_BUCKET_BYTES,
            MIN_PADDING_BUCKET_BYTES * 2,
            POWER_OF_TWO_PADDING_LIMIT_BYTES,
            POWER_OF_TWO_PADDING_LIMIT_BYTES + LARGE_PADDING_BUCKET_STEP_BYTES,
            MAX_PADDING_BUCKET_BYTES,
        ] {
            assert_eq!(validate_authenticated_padding_bucket(size), Ok(()));
        }
    }

    #[test]
    fn authenticated_padding_bucket_rejects_out_of_bounds_sizes() {
        for size in [
            0,
            MIN_PADDING_BUCKET_BYTES - 1,
            MAX_PADDING_BUCKET_BYTES + 1,
        ] {
            assert_eq!(
                validate_authenticated_padding_bucket(size),
                Err(Error::OutsideBounds)
            );
        }
    }

    #[test]
    fn authenticated_padding_bucket_rejects_unsupported_small_buckets() {
        assert_eq!(
            validate_authenticated_padding_bucket(MIN_PADDING_BUCKET_BYTES * 3),
            Err(Error::UnsupportedPowerOfTwo)
        );
    }

    #[test]
    fn authenticated_padding_bucket_rejects_misaligned_large_buckets() {
        for size in [
            POWER_OF_TWO_PADDING_LIMIT_BYTES + 1,
            POWER_OF_TWO_PADDING_LIMIT_BYTES + LARGE_PADDING_BUCKET_STEP_BYTES - 1,
        ] {
            assert_eq!(
                validate_authenticated_padding_bucket(size),
                Err(Error::MisalignedLargeBucket)
            );
        }
    }
}
