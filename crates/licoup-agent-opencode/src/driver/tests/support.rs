use super::PathBuf;

pub(super) fn absolute_test_cwd() -> PathBuf {
    std::env::current_dir().expect("test working directory")
}

/// Install the standing test host this package's socket-level claims run against.
///
/// These tests exercise this Agent's own session and stream logic over the same
/// bounded engine the client installs, so the port is answered here by that
/// engine's real HTTP reader, SSE framer, byte rule and active-turn registry
/// rather than by a second implementation of them. Installation is first-wins
/// and every caller tolerates a repeat, so the whole test binary shares one
/// host.
pub(super) fn install_standing_host() {
    use crate::port::serve::{
        ServeAttachment, ServeByteDirection, ServeFramingFailure, ServePort, ServeRequestFailure,
        ServeTurnAdmission, ServeTurnGuard,
    };
    use licoup_agent_drivers::local_service::{http, sse, turn_control};
    use licoup_foundation::platform::raw_execution::{RawExecutionDirection, RawExecutionObserver};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    fn request_failure(failure: http::HttpFailure) -> ServeRequestFailure {
        match failure {
            http::HttpFailure::BodyTooLarge => ServeRequestFailure::BodyTooLarge,
            http::HttpFailure::Busy => ServeRequestFailure::Busy,
            http::HttpFailure::HeadersTooLarge => ServeRequestFailure::HeadersTooLarge,
            http::HttpFailure::InvalidJson => ServeRequestFailure::InvalidJson,
            http::HttpFailure::InvalidUrl => ServeRequestFailure::InvalidUrl,
            http::HttpFailure::NotFound => ServeRequestFailure::NotFound,
            http::HttpFailure::Request => ServeRequestFailure::Request,
            http::HttpFailure::Serialize => ServeRequestFailure::Serialize,
            http::HttpFailure::Status(status) => ServeRequestFailure::Status(status),
            http::HttpFailure::Unavailable => ServeRequestFailure::Unavailable,
        }
    }

    fn framing_failure(failure: sse::SseFailure) -> ServeFramingFailure {
        match failure {
            sse::SseFailure::Busy => ServeFramingFailure::Busy,
            sse::SseFailure::EventLimit => ServeFramingFailure::EventLimit,
            sse::SseFailure::FrameTooLarge => ServeFramingFailure::FrameTooLarge,
            sse::SseFailure::HeadersTooLarge => ServeFramingFailure::HeadersTooLarge,
            sse::SseFailure::InvalidUtf8 => ServeFramingFailure::InvalidUtf8,
            sse::SseFailure::InvalidUrl => ServeFramingFailure::InvalidUrl,
            sse::SseFailure::LineTooLarge => ServeFramingFailure::LineTooLarge,
            sse::SseFailure::Request => ServeFramingFailure::Request,
            sse::SseFailure::Unavailable => ServeFramingFailure::Unavailable,
        }
    }

    fn ensure_attachment(_executable: &str) -> Result<ServeAttachment, String> {
        Err("the standing test host attaches no endpoint".to_owned())
    }

    fn get_json(url: &str, observed: bool) -> Result<serde_json::Value, ServeRequestFailure> {
        let source = observed.then_some("opencode.http");
        match source {
            Some(source) => http::get_json_observed(url, Duration::from_secs(5), source),
            None => http::get_json(url, Duration::from_secs(5)),
        }
        .map_err(request_failure)
    }

    fn post_json(
        url: &str,
        body: &serde_json::Value,
        timeout: Option<Duration>,
    ) -> Result<serde_json::Value, ServeRequestFailure> {
        http::post_json_observed(url, body, timeout, "opencode.http").map_err(request_failure)
    }

    fn watch_frames(
        url: &str,
        stop: &AtomicBool,
        on_frame: &mut dyn FnMut(&str, &str) -> bool,
    ) -> Result<(), ServeFramingFailure> {
        let observer = RawExecutionObserver::current();
        let _scope = licoup_foundation::platform::raw_execution::RawExecutionScope::enter(observer);
        sse::watch_frames(url, stop, |data, frame| {
            let frame = String::from_utf8_lossy(frame);
            on_frame(data, &frame)
        })
        .map_err(framing_failure)
    }

    fn observe_bytes(
        source: &str,
        direction: ServeByteDirection,
        session_id: Option<&str>,
        payload: &str,
        frame: &str,
    ) {
        let Some(observer) = RawExecutionObserver::current() else {
            return;
        };
        if let Some(session_id) = session_id
            && !sse::frame_belongs_to_session(payload, session_id)
        {
            return;
        }
        let direction = match direction {
            ServeByteDirection::Received => RawExecutionDirection::Received,
            ServeByteDirection::Sent => RawExecutionDirection::Sent,
        };
        observer.record_bytes(source, direction, frame.as_bytes());
    }

    fn admit_turn(
        attach_url: &str,
        session_id: &str,
        on_failure: Option<crate::port::serve::ServeControlFailureObserver>,
    ) -> ServeTurnAdmission {
        let observer = on_failure.map(|observer| {
            let observer: std::sync::Arc<dyn Fn(http::HttpFailure) + Send + Sync> =
                std::sync::Arc::new(move |failure| observer(request_failure(failure)));
            observer
        });
        match turn_control::register("opencode-serve", attach_url, session_id, observer) {
            Ok(guard) => ServeTurnAdmission::Admitted(ServeTurnGuard::held(guard)),
            Err(()) => ServeTurnAdmission::AtCapacity,
        }
    }

    let _ = crate::port::serve::install(ServePort {
        ensure_attachment,
        get_json,
        post_json,
        watch_frames,
        observe_bytes,
        admit_turn,
    });
}
