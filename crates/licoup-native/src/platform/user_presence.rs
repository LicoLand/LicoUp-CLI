//! App-session platform user-presence authorization.
//!
//! The native host authenticates its user once and reuses that retained OS
//! context for exact, domain-scoped grants. Touch ID is mandatory when it is
//! available; the macOS account password is used only when biometry cannot be
//! used on the device. Secret operations remain independently budgeted and
//! bound by their domain grants.

#[cfg(target_os = "macos")]
use anyhow::ensure;
use anyhow::{Result, anyhow};

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use core::ffi::c_void;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation_sys::base::CFTypeRef;
    use objc2::rc::Retained;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAError, LAPolicy};
    use std::sync::{Arc, Mutex, OnceLock, mpsc};

    #[derive(Clone)]
    pub(crate) struct Session {
        context: Retained<LAContext>,
        effect_lock: Arc<Mutex<()>>,
    }

    // SAFETY: the retained context is immutable after authorization and is
    // passed only to synchronous Security.framework calls.
    unsafe impl Send for Session {}
    // SAFETY: no shared reference mutates LAContext after construction.
    unsafe impl Sync for Session {}

    static APPLICATION_AUTHORIZATION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();

    fn application_authorization() -> &'static Mutex<Option<Session>> {
        APPLICATION_AUTHORIZATION.get_or_init(|| Mutex::new(None))
    }

    impl Session {
        pub(crate) fn as_cf_type(&self) -> CFType {
            let pointer = (&*self.context as *const LAContext).cast::<c_void>() as CFTypeRef;
            // SAFETY: `self.context` retains the object; get-rule wrapping does
            // not consume it.
            unsafe { CFType::wrap_under_get_rule(pointer) }
        }

        pub(crate) fn effect_lock(&self) -> Arc<Mutex<()>> {
            Arc::clone(&self.effect_lock)
        }
    }

    pub(crate) fn available() -> bool {
        if cfg!(test) {
            return false;
        }
        // SAFETY: LAContext::new returns a retained initialized context.
        let context = unsafe { LAContext::new() };
        preferred_policy(&context).is_ok()
    }

    pub(crate) fn authorize(reason: &str, scope: &str) -> Result<Session> {
        ensure!(
            reason == reason.trim()
                && !reason.is_empty()
                && reason.len() <= 768
                && !reason.chars().any(char::is_control)
                && scope == scope.trim()
                && !scope.is_empty()
                && scope.len() <= 2_048
                && !scope.chars().any(char::is_control),
            "user_presence_authorization_scope_invalid"
        );
        let mut authorization = application_authorization()
            .lock()
            .map_err(|_| anyhow!("user_presence_authorization_unavailable"))?;
        if let Some(session) = authorization.as_ref() {
            return Ok(session.clone());
        }

        // SAFETY: LAContext::new returns a retained initialized context.
        let context = unsafe { LAContext::new() };
        let reason = NSString::from_str(reason);
        unsafe {
            context.setLocalizedReason(&reason);
            context.setInteractionNotAllowed(false);
            context.setTouchIDAuthenticationAllowableReuseDuration(0.0);
        }
        let policy = preferred_policy(&context)?;
        if policy == LAPolicy::DeviceOwnerAuthenticationWithBiometrics {
            let no_password_fallback = NSString::from_str("");
            // SAFETY: both retained objects remain live through evaluation.
            unsafe { context.setLocalizedFallbackTitle(Some(&no_password_fallback)) };
        }
        let (sender, receiver) = mpsc::channel();
        let reply =
            block2::RcBlock::new(move |success: objc2::runtime::Bool, error: *mut NSError| {
                let error_code = if error.is_null() {
                    None
                } else {
                    // SAFETY: LocalAuthentication owns the NSError for the
                    // callback duration; only its scalar code is copied.
                    Some(unsafe { (*error).code() })
                };
                let _ = sender.send((success.as_bool(), error_code));
            });
        // SAFETY: the retained context, reason, and reply remain live through
        // submission; the block retains its captured channel sender.
        unsafe { context.evaluatePolicy_localizedReason_reply(policy, &reason, &reply) };
        // The OS and user own authentication completion and cancellation.
        // A slow response must not invalidate the pending native prompt.
        match receiver.recv() {
            Ok((true, _)) => {}
            Ok((false, code)) => {
                return Err(anyhow!(
                    "user_presence_authorization_failed:{}",
                    error_category(code)
                ));
            }
            Err(_) => return Err(anyhow!("user_presence_authorization_callback_missing")),
        }
        // Keychain calls may use this already-authorized context but may never
        // open another interactive prompt.
        unsafe { context.setInteractionNotAllowed(true) };
        let session = Session {
            context,
            effect_lock: Arc::new(Mutex::new(())),
        };
        *authorization = Some(session.clone());
        Ok(session)
    }

    fn preferred_policy(context: &LAContext) -> Result<LAPolicy> {
        // SAFETY: context is retained and both policies are SDK-defined.
        let biometric_result = unsafe {
            context.canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthenticationWithBiometrics)
        };
        match biometric_result {
            Ok(()) => Ok(LAPolicy::DeviceOwnerAuthenticationWithBiometrics),
            Err(error) if password_fallback_allowed(error.code()) => {
                // Password is the fallback only when no usable biometric is
                // available or enrolled. A lockout or rejected fingerprint
                // never silently changes authentication mechanisms.
                unsafe {
                    context
                        .canEvaluatePolicy_error(LAPolicy::DeviceOwnerAuthentication)
                        .map_err(|_| anyhow!("user_presence_authorization_unavailable"))?;
                }
                Ok(LAPolicy::DeviceOwnerAuthentication)
            }
            Err(error) => Err(anyhow!(
                "user_presence_authorization_unavailable:{}",
                error_category(Some(error.code()))
            )),
        }
    }

    fn password_fallback_allowed(code: isize) -> bool {
        code == LAError::BiometryNotAvailable.0
            || code == LAError::BiometryNotEnrolled.0
            || code == LAError::BiometryNotPaired.0
            || code == LAError::BiometryDisconnected.0
    }

    pub(crate) fn invalidate() {
        if let Ok(mut authorization) = application_authorization().lock()
            && let Some(session) = authorization.take()
        {
            // SAFETY: invalidation is the documented cancellation and
            // revocation operation for a retained LAContext.
            unsafe { session.context.invalidate() };
        }
    }

    fn error_category(code: Option<isize>) -> &'static str {
        match code {
            Some(value) if value == LAError::UserCancel.0 => "user_cancelled",
            Some(value) if value == LAError::UserFallback.0 => "password_fallback_blocked",
            Some(value) if value == LAError::SystemCancel.0 => "system_cancelled",
            Some(value) if value == LAError::AppCancel.0 => "application_cancelled",
            Some(value) if value == LAError::BiometryLockout.0 => "biometry_locked",
            Some(value) if value == LAError::BiometryNotAvailable.0 => "biometry_unavailable",
            Some(value) if value == LAError::BiometryNotEnrolled.0 => "biometry_not_enrolled",
            Some(value) if value == LAError::PasscodeNotSet.0 => "credential_unavailable",
            Some(value) if value == LAError::AuthenticationFailed.0 => "authentication_failed",
            _ => "authorization_unavailable",
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        struct AuthorizationReset;

        impl Drop for AuthorizationReset {
            fn drop(&mut self) {
                invalidate();
            }
        }

        #[test]
        fn password_fallback_is_limited_to_missing_biometry() {
            for code in [
                LAError::BiometryNotAvailable.0,
                LAError::BiometryNotEnrolled.0,
                LAError::BiometryNotPaired.0,
                LAError::BiometryDisconnected.0,
            ] {
                assert!(password_fallback_allowed(code));
            }
            for code in [
                LAError::BiometryLockout.0,
                LAError::AuthenticationFailed.0,
                LAError::UserCancel.0,
                LAError::UserFallback.0,
            ] {
                assert!(!password_fallback_allowed(code));
            }
        }

        #[test]
        fn application_authorization_reuses_one_native_context_until_revoked() {
            let _reset = AuthorizationReset;
            invalidate();
            // SAFETY: this test never evaluates the context or displays UI.
            let context = unsafe { LAContext::new() };
            let session = Session {
                context,
                effect_lock: Arc::new(Mutex::new(())),
            };
            *application_authorization().lock().unwrap() = Some(session);
            let first = application_authorization().lock().unwrap().clone().unwrap();
            let second = application_authorization().lock().unwrap().clone().unwrap();
            assert!(std::ptr::eq::<LAContext>(&*first.context, &*second.context));
            assert!(Arc::ptr_eq(&first.effect_lock, &second.effect_lock));

            invalidate();
            assert!(application_authorization().lock().unwrap().is_none());
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::Session as UserPresenceSession;

#[cfg(not(target_os = "macos"))]
#[derive(Clone)]
pub(crate) struct UserPresenceSession;

pub(crate) fn available() -> bool {
    #[cfg(target_os = "macos")]
    {
        macos::available()
    }
    #[cfg(not(target_os = "macos"))]
    {
        false
    }
}

pub(crate) fn authorize(reason: &str, scope: &str) -> Result<UserPresenceSession> {
    #[cfg(target_os = "macos")]
    {
        macos::authorize(reason, scope)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (reason, scope);
        Err(anyhow!("user_presence_authorization_unavailable"))
    }
}

#[cfg(target_os = "macos")]
pub(crate) fn invalidate() {
    macos::invalidate();
}
