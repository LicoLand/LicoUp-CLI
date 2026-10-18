//! The only BadTower station network-I/O boundary.

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;

use super::contract::{
    BadTowerStationError, BadTowerStationErrorCategory, BadTowerStationOperation,
    HTTP_TIMEOUT_SECONDS, MAX_ERROR_RESPONSE_BYTES,
};
use super::wire::{ErrorResponse, StationErrorCode};

pub(super) struct StationHttpClient {
    base_url: String,
    agent: ureq::Agent,
}

impl StationHttpClient {
    pub(super) fn new(base_url: String) -> Self {
        Self {
            base_url,
            agent: ureq::AgentBuilder::new()
                .timeout(Duration::from_secs(HTTP_TIMEOUT_SECONDS))
                .redirects(0)
                .build(),
        }
    }

    pub(super) fn post_json<T>(
        &self,
        operation: BadTowerStationOperation,
        path: &str,
        request_body: &str,
        expected_status: u16,
        response_limit: usize,
    ) -> Result<T, BadTowerStationError>
    where
        T: DeserializeOwned,
    {
        let request = self
            .agent
            .post(&self.url(path))
            .set("accept", "application/json")
            .set("content-type", "application/json");
        self.execute(
            operation,
            request.send_string(request_body),
            expected_status,
            response_limit,
        )
    }

    pub(super) fn get_json<T>(
        &self,
        operation: BadTowerStationOperation,
        path: &str,
        expected_status: u16,
        response_limit: usize,
    ) -> Result<T, BadTowerStationError>
    where
        T: DeserializeOwned,
    {
        let request = self
            .agent
            .get(&self.url(path))
            .set("accept", "application/json");
        self.execute(operation, request.call(), expected_status, response_limit)
    }

    pub(super) fn delete_json<T>(
        &self,
        operation: BadTowerStationOperation,
        path: &str,
        expected_status: u16,
        response_limit: usize,
    ) -> Result<T, BadTowerStationError>
    where
        T: DeserializeOwned,
    {
        let request = self
            .agent
            .delete(&self.url(path))
            .set("accept", "application/json");
        self.execute(operation, request.call(), expected_status, response_limit)
    }

    fn execute<T>(
        &self,
        operation: BadTowerStationOperation,
        result: Result<ureq::Response, ureq::Error>,
        expected_status: u16,
        response_limit: usize,
    ) -> Result<T, BadTowerStationError>
    where
        T: DeserializeOwned,
    {
        match result {
            Ok(response) => {
                if response.status() != expected_status {
                    return Err(error(
                        operation,
                        BadTowerStationErrorCategory::ResponseProtocol,
                        false,
                    ));
                }
                decode_json(operation, response, response_limit)
            }
            Err(ureq::Error::Status(status, response)) => {
                Err(decode_station_error(operation, status, response))
            }
            Err(ureq::Error::Transport(_)) => Err(error(
                operation,
                BadTowerStationErrorCategory::TransportOutcomeUnknown,
                false,
            )),
        }
    }

    fn url(&self, path: &str) -> String {
        let mut url = String::with_capacity(self.base_url.len() + path.len());
        url.push_str(&self.base_url);
        url.push_str(path);
        url
    }
}

fn decode_json<T>(
    operation: BadTowerStationOperation,
    response: ureq::Response,
    limit: usize,
) -> Result<T, BadTowerStationError>
where
    T: DeserializeOwned,
{
    if !is_json_media_type(response.header("content-type")) {
        return Err(error(
            operation,
            BadTowerStationErrorCategory::ResponseProtocol,
            false,
        ));
    }
    let bytes = read_bounded(operation, response, limit)?;
    serde_json::from_slice(&bytes).map_err(|_| {
        error(
            operation,
            BadTowerStationErrorCategory::ResponseProtocol,
            false,
        )
    })
}

fn decode_station_error(
    operation: BadTowerStationOperation,
    status: u16,
    response: ureq::Response,
) -> BadTowerStationError {
    let Ok(problem) = decode_json::<ErrorResponse>(operation, response, MAX_ERROR_RESPONSE_BYTES)
    else {
        return error(
            operation,
            BadTowerStationErrorCategory::ResponseProtocol,
            false,
        );
    };
    let (category, retryable) = match (status, problem.error.code) {
        (400, StationErrorCode::InvalidRequest) => {
            (BadTowerStationErrorCategory::StationRejectedInput, false)
        }
        (409, StationErrorCode::LeaseRequired) => {
            (BadTowerStationErrorCategory::LeaseRequired, false)
        }
        (409, StationErrorCode::TransportConflict) => {
            (BadTowerStationErrorCategory::TransportConflict, false)
        }
        (429, StationErrorCode::StationLimit) => {
            (BadTowerStationErrorCategory::StationCapacity, true)
        }
        (500, StationErrorCode::InternalError) => {
            (BadTowerStationErrorCategory::StationFailure, true)
        }
        _ => (BadTowerStationErrorCategory::ResponseProtocol, false),
    };
    error(operation, category, retryable)
}

fn read_bounded(
    operation: BadTowerStationOperation,
    response: ureq::Response,
    limit: usize,
) -> Result<Vec<u8>, BadTowerStationError> {
    if response
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > limit)
    {
        return Err(error(
            operation,
            BadTowerStationErrorCategory::ResponseTooLarge,
            false,
        ));
    }
    let take_limit = u64::try_from(limit)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| {
            error(
                operation,
                BadTowerStationErrorCategory::ResponseTooLarge,
                false,
            )
        })?;
    let mut bytes = Vec::with_capacity(limit.min(16 * 1024));
    response
        .into_reader()
        .take(take_limit)
        .read_to_end(&mut bytes)
        .map_err(|_| {
            error(
                operation,
                BadTowerStationErrorCategory::ResponseOutcomeUnknown,
                false,
            )
        })?;
    if bytes.len() > limit {
        return Err(error(
            operation,
            BadTowerStationErrorCategory::ResponseTooLarge,
            false,
        ));
    }
    Ok(bytes)
}

fn is_json_media_type(value: Option<&str>) -> bool {
    value
        .and_then(|value| value.split(';').next())
        .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
}

pub(super) const fn error(
    operation: BadTowerStationOperation,
    category: BadTowerStationErrorCategory,
    retryable: bool,
) -> BadTowerStationError {
    BadTowerStationError::new(operation, category, retryable)
}
