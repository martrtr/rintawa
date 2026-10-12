//! Owner-scoped nonblocking HTTP operations for sandboxed extensions.
//!
//! HTTPS requests run on bounded host worker threads. The guest observes ordered,
//! bounded response chunks through a polling interface; a callback never blocks on
//! DNS, TLS, upload or a remote provider. No redirects are followed.

use std::{
    io::Read,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use reqwest::{
    Method,
    blocking::Response,
    header::{HeaderMap, HeaderName, HeaderValue},
};
use rintawa_sdk::{contracts::ComponentRef, runtime_permissions::RuntimePermission};
use url::Url;

use crate::{
    runtime::wasm::{
        HttpFetchError, HttpRequestError, HttpRequestHost, RuntimePermissionCheck, WasmHostState,
        WitHttpEvent, WitHttpHeader, WitHttpRequest, WitHttpResponseHead,
        network::build_bounded_http_client,
    },
    runtime_permissions::is_local_http_origin,
};

const HTTP_WORKER_CHANNEL_CAPACITY: usize = 4;
const MAX_GLOBAL_HTTP_WORKERS: usize = 64;
static GLOBAL_HTTP_WORKERS: AtomicUsize = AtomicUsize::new(0);

/// A process-wide worker slot bounds total OS threads across all World instances.
struct HttpWorkerPermit {
    counter: &'static AtomicUsize,
}

impl HttpWorkerPermit {
    fn acquire() -> Result<Self, HttpRequestError> {
        Self::acquire_from(&GLOBAL_HTTP_WORKERS, MAX_GLOBAL_HTTP_WORKERS)
    }

    fn acquire_from(counter: &'static AtomicUsize, limit: usize) -> Result<Self, HttpRequestError> {
        let mut observed = counter.load(Ordering::Acquire);
        loop {
            if observed >= limit {
                return Err(HttpRequestError::LimitExceeded);
            }
            match counter.compare_exchange_weak(
                observed,
                observed + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Ok(Self { counter }),
                Err(actual) => observed = actual,
            }
        }
    }
}

impl Drop for HttpWorkerPermit {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::AcqRel);
    }
}

const HTTP_WORKER_READ_BYTES: usize = 8 * 1024;
const HTTP_WORKER_BACKPRESSURE_WAIT: Duration = Duration::from_millis(5);

/// A worker event never contains application secrets in diagnostic text.
enum WorkerEvent {
    Head(WitHttpResponseHead),
    Chunk(Vec<u8>),
    Completed,
    Failed(HttpRequestError),
}

/// Each handle is bound to its initiating component principal.
pub(super) struct OwnedHttpRequest {
    pub(super) owner: ComponentRef,
    receiver: Receiver<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
    local_origin: Option<Url>,
    pending_chunk: Vec<u8>,
    deadline: Instant,
    completed: bool,
    failure: Option<HttpRequestError>,
}

impl Drop for OwnedHttpRequest {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct ValidatedRequest {
    local_origin_access: Option<(
        ComponentRef,
        crate::runtime_permissions::RuntimePermissionManager,
    )>,
    method: Method,
    url: Url,
    headers: HeaderMap,
    body: Vec<u8>,
    response_limit: usize,
    max_response_headers: usize,
    max_response_header_bytes: usize,
}

#[derive(Clone, Copy)]
struct HttpLimits {
    max_body_bytes: usize,
    max_headers: usize,
    max_header_bytes: usize,
    max_response_bytes: usize,
}

impl WasmHostState {
    fn current_http_owner(&mut self) -> Result<ComponentRef, HttpRequestError> {
        if !self.http_request_access_active {
            return Err(HttpRequestError::AccessNotActive);
        }
        match self.runtime_permission_owner(RuntimePermission::HttpRequest) {
            Ok(owner) => Ok(owner),
            Err(RuntimePermissionCheck::Denied) => {
                // Revocation must tear down any previously running worker owned by
                // this exact principal, even if it can no longer poll or close.
                if let Some(owner) = self.current_execution_owner().cloned() {
                    self.http_requests
                        .retain(|_, request| request.owner != owner);
                }
                Err(HttpRequestError::PermissionDenied)
            }
            Err(RuntimePermissionCheck::Unavailable) => Err(HttpRequestError::Unavailable),
        }
    }

    fn http_limits(&self) -> HttpLimits {
        HttpLimits {
            max_body_bytes: self.max_http_request_body_bytes,
            max_headers: self.max_http_request_headers,
            max_header_bytes: self.max_http_request_header_bytes,
            max_response_bytes: self.max_http_fetch_bytes,
        }
    }

    fn allocate_http_request_handle(
        &mut self,
        owner: ComponentRef,
        receiver: Receiver<WorkerEvent>,
        cancelled: Arc<AtomicBool>,
        local_origin: Option<Url>,
    ) -> Result<u64, HttpRequestError> {
        let owned_count = self
            .http_requests
            .values()
            .filter(|request| request.owner == owner)
            .count();
        if owned_count >= self.max_http_request_handles {
            return Err(HttpRequestError::LimitExceeded);
        }
        let handle = self.next_http_request_handle;
        self.next_http_request_handle =
            handle.checked_add(1).ok_or(HttpRequestError::Unavailable)?;
        self.http_requests.insert(
            handle,
            OwnedHttpRequest {
                owner,
                receiver,
                cancelled,
                local_origin,
                pending_chunk: Vec::new(),
                deadline: Instant::now() + Duration::from_millis(self.http_fetch_timeout_ms),
                completed: false,
                failure: None,
            },
        );
        Ok(handle)
    }
}

impl HttpRequestHost for WasmHostState {
    fn start(&mut self, request: WitHttpRequest) -> Result<u64, HttpRequestError> {
        let owner = self.current_http_owner()?;
        if self
            .http_requests
            .values()
            .filter(|handle| handle.owner == owner)
            .count()
            >= self.max_http_request_handles
        {
            return Err(HttpRequestError::LimitExceeded);
        }

        let mut validated = validate_request(request, self.http_limits())?;
        validated.local_origin_access = Some((owner.clone(), self.runtime_permissions.clone()));
        if validated.url.scheme() == "http"
            && !self
                .runtime_permissions
                .has_http_loopback_origin(&owner, &validated.url)
                .map_err(|()| HttpRequestError::Unavailable)?
        {
            return Err(HttpRequestError::ForbiddenDestination);
        }
        let (sender, receiver) = mpsc::sync_channel(HTTP_WORKER_CHANNEL_CAPACITY);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = Arc::clone(&cancelled);
        let timeout = Duration::from_millis(self.http_fetch_timeout_ms);
        let permit = HttpWorkerPermit::acquire()?;

        let local_origin = (validated.url.scheme() == "http").then(|| validated.url.clone());

        thread::Builder::new()
            .name(String::from("rintawa-http-request"))
            .spawn(move || {
                let _permit = permit;
                run_http_worker(validated, timeout, sender, worker_cancelled);
            })
            .map_err(|_| HttpRequestError::Unavailable)?;

        self.allocate_http_request_handle(owner, receiver, cancelled, local_origin)
    }

    fn poll(&mut self, handle: u64, max_bytes: u32) -> Result<WitHttpEvent, HttpRequestError> {
        let owner = self.current_http_owner()?;
        let read_limit = usize::try_from(max_bytes)
            .unwrap_or(usize::MAX)
            .min(self.max_http_request_chunk_bytes);
        if read_limit == 0 {
            return Err(HttpRequestError::LimitExceeded);
        }
        let Some(request) = self.http_requests.get_mut(&handle) else {
            return Err(HttpRequestError::UnknownHandle);
        };
        if request.owner != owner {
            return Err(HttpRequestError::UnknownHandle);
        }
        // A revoked origin cannot be used to drain response bytes previously
        // queued by an in-flight HTTP worker.
        if let Some(origin) = &request.local_origin {
            let is_authorized = self
                .runtime_permissions
                .has_http_loopback_origin(&owner, origin)
                .map_err(|()| HttpRequestError::Unavailable)?;
            if !is_authorized {
                request.cancelled.store(true, Ordering::Release);
                request.pending_chunk.clear();
                request.failure = Some(HttpRequestError::PermissionDenied);
            }
        }
        if !request.completed && request.failure.is_none() && Instant::now() >= request.deadline {
            request.cancelled.store(true, Ordering::Release);
            request.pending_chunk.clear();
            request.failure = Some(HttpRequestError::Timeout);
        }

        if !request.pending_chunk.is_empty() {
            return Ok(read_chunk(request, read_limit));
        }
        if let Some(failure) = request.failure {
            return Ok(WitHttpEvent::Failed(failure));
        }
        if request.completed {
            return Ok(WitHttpEvent::Completed);
        }
        match request.receiver.try_recv() {
            Ok(WorkerEvent::Head(head)) => Ok(WitHttpEvent::Response(head)),
            Ok(WorkerEvent::Chunk(chunk)) => {
                request.pending_chunk = chunk;
                Ok(read_chunk(request, read_limit))
            }
            Ok(WorkerEvent::Completed) => {
                request.completed = true;
                Ok(WitHttpEvent::Completed)
            }
            Ok(WorkerEvent::Failed(error)) => {
                request.failure = Some(error);
                Ok(WitHttpEvent::Failed(error))
            }
            Err(TryRecvError::Empty) => Ok(WitHttpEvent::Pending),
            Err(TryRecvError::Disconnected) => {
                request.failure = Some(HttpRequestError::Unavailable);
                Ok(WitHttpEvent::Failed(HttpRequestError::Unavailable))
            }
        }
    }

    fn close(&mut self, handle: u64) -> Result<(), HttpRequestError> {
        let owner = self.current_http_owner()?;
        let Some(request) = self.http_requests.get(&handle) else {
            return Err(HttpRequestError::UnknownHandle);
        };
        if request.owner != owner {
            return Err(HttpRequestError::UnknownHandle);
        }
        self.http_requests.remove(&handle);
        Ok(())
    }
}

fn read_chunk(request: &mut OwnedHttpRequest, max_bytes: usize) -> WitHttpEvent {
    let read_bytes = max_bytes.min(request.pending_chunk.len());
    let bytes: Vec<_> = request.pending_chunk.drain(..read_bytes).collect();
    WitHttpEvent::Data(bytes)
}

fn validate_request(
    request: WitHttpRequest,
    limits: HttpLimits,
) -> Result<ValidatedRequest, HttpRequestError> {
    let method = match request.method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        "PUT" => Method::PUT,
        "PATCH" => Method::PATCH,
        "DELETE" => Method::DELETE,
        "HEAD" => Method::HEAD,
        _ => return Err(HttpRequestError::InvalidMethod),
    };
    let url = Url::parse(&request.url).map_err(|_| HttpRequestError::InvalidUrl)?;
    if (url.scheme() != "https" && !is_local_http_origin(&url))
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(HttpRequestError::InvalidUrl);
    }
    if request.body.len() > limits.max_body_bytes {
        return Err(HttpRequestError::RequestTooLarge);
    }
    if request.headers.len() > limits.max_headers {
        return Err(HttpRequestError::InvalidHeader);
    }
    let response_limit = usize::try_from(request.max_response_bytes)
        .unwrap_or(usize::MAX)
        .min(limits.max_response_bytes);
    if response_limit == 0 {
        return Err(HttpRequestError::LimitExceeded);
    }

    let mut headers = HeaderMap::new();
    let mut total_header_bytes = 0usize;
    for header in request.headers {
        total_header_bytes = total_header_bytes
            .checked_add(header.name.len())
            .and_then(|length| length.checked_add(header.value.len()))
            .ok_or(HttpRequestError::InvalidHeader)?;
        if total_header_bytes > limits.max_header_bytes {
            return Err(HttpRequestError::InvalidHeader);
        }
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| HttpRequestError::InvalidHeader)?;
        if is_forbidden_header(&name) || headers.contains_key(&name) {
            return Err(HttpRequestError::InvalidHeader);
        }
        let value =
            HeaderValue::from_str(&header.value).map_err(|_| HttpRequestError::InvalidHeader)?;
        headers.insert(name, value);
    }
    Ok(ValidatedRequest {
        local_origin_access: None,
        max_response_headers: limits.max_headers,
        max_response_header_bytes: limits.max_header_bytes,
        method,
        url,
        headers,
        body: request.body,
        response_limit,
    })
}

fn is_forbidden_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "host"
            | "connection"
            | "content-length"
            | "transfer-encoding"
            | "proxy-authorization"
            | "proxy-connection"
            | "upgrade"
            | "trailer"
            | "te"
            | "keep-alive"
            | "cookie"
    )
}

fn run_http_worker(
    request: ValidatedRequest,
    timeout: Duration,
    sender: SyncSender<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
) {
    let deadline = Instant::now() + timeout;
    let outcome = perform_http_request(&request, timeout, deadline, &sender, &cancelled);
    if !cancelled.load(Ordering::Acquire) {
        let terminal = match outcome {
            Ok(()) => WorkerEvent::Completed,
            Err(error) => WorkerEvent::Failed(error),
        };
        let _ = send_event(&sender, &cancelled, terminal, deadline);
    }
}

fn perform_http_request(
    request: &ValidatedRequest,
    timeout: Duration,
    deadline: Instant,
    sender: &SyncSender<WorkerEvent>,
    cancelled: &AtomicBool,
) -> Result<(), HttpRequestError> {
    if cancelled.load(Ordering::Acquire) {
        return Err(HttpRequestError::Unavailable);
    }
    let is_loopback_http = request.url.scheme() == "http";
    if is_loopback_http && !has_active_local_grant(request)? {
        return Err(HttpRequestError::ForbiddenDestination);
    }
    let client = build_bounded_http_client(&request.url, timeout, is_loopback_http)
        .map_err(map_fetch_error)?;
    if cancelled.load(Ordering::Acquire) {
        return Err(HttpRequestError::Unavailable);
    }
    if is_loopback_http && !has_active_local_grant(request)? {
        return Err(HttpRequestError::PermissionDenied);
    }
    let mut response = client
        .request(request.method.clone(), request.url.clone())
        .headers(request.headers.clone())
        .body(request.body.clone())
        .send()
        .map_err(map_transport_error)?;

    if response
        .content_length()
        .is_some_and(|length| length > request.response_limit as u64)
    {
        return Err(HttpRequestError::ResponseTooLarge);
    }
    let head = response_head(
        &response,
        request.max_response_headers,
        request.max_response_header_bytes,
    )?;
    if !send_event(sender, cancelled, WorkerEvent::Head(head), deadline) {
        return Err(HttpRequestError::Unavailable);
    }

    let mut total_bytes = 0usize;
    let mut buffer = [0u8; HTTP_WORKER_READ_BYTES];
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err(HttpRequestError::Unavailable);
        }
        if is_loopback_http && !has_active_local_grant(request)? {
            return Err(HttpRequestError::PermissionDenied);
        }
        if Instant::now() >= deadline {
            return Err(HttpRequestError::Timeout);
        }
        let read = response.read(&mut buffer).map_err(|error| {
            if error.kind() == std::io::ErrorKind::TimedOut {
                HttpRequestError::Timeout
            } else {
                HttpRequestError::Unavailable
            }
        })?;
        if read == 0 {
            return Ok(());
        }
        total_bytes = total_bytes
            .checked_add(read)
            .ok_or(HttpRequestError::ResponseTooLarge)?;
        if total_bytes > request.response_limit {
            return Err(HttpRequestError::ResponseTooLarge);
        }
        if !send_event(
            sender,
            cancelled,
            WorkerEvent::Chunk(buffer[..read].to_vec()),
            deadline,
        ) {
            return Err(HttpRequestError::Unavailable);
        }
    }
}

fn has_active_local_grant(request: &ValidatedRequest) -> Result<bool, HttpRequestError> {
    let Some((owner, policy)) = &request.local_origin_access else {
        return Ok(false);
    };
    policy
        .has_http_loopback_origin(owner, &request.url)
        .map_err(|()| HttpRequestError::Unavailable)
}

fn response_head(
    response: &Response,
    max_headers: usize,
    max_header_bytes: usize,
) -> Result<WitHttpResponseHead, HttpRequestError> {
    let mut headers = Vec::new();
    let mut total_bytes = 0usize;
    for (name, value) in response.headers() {
        // Secret-bearing response headers must never reach guest-visible diagnostics.
        if matches!(name.as_str(), "set-cookie" | "proxy-authenticate") {
            continue;
        }
        let Ok(value) = value.to_str() else {
            continue;
        };
        total_bytes = total_bytes
            .checked_add(name.as_str().len())
            .and_then(|length| length.checked_add(value.len()))
            .ok_or(HttpRequestError::ResponseTooLarge)?;
        if headers.len() >= max_headers || total_bytes > max_header_bytes {
            return Err(HttpRequestError::ResponseTooLarge);
        }
        headers.push(WitHttpHeader {
            name: name.to_string(),
            value: value.to_owned(),
        });
    }
    Ok(WitHttpResponseHead {
        status: response.status().as_u16(),
        headers,
    })
}

fn send_event(
    sender: &SyncSender<WorkerEvent>,
    cancelled: &AtomicBool,
    mut event: WorkerEvent,
    deadline: Instant,
) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) || Instant::now() >= deadline {
            return false;
        }
        match sender.try_send(event) {
            Ok(()) => return true,
            Err(TrySendError::Disconnected(_)) => return false,
            Err(TrySendError::Full(pending)) => {
                event = pending;
                thread::sleep(HTTP_WORKER_BACKPRESSURE_WAIT);
            }
        }
    }
}

fn map_fetch_error(error: HttpFetchError) -> HttpRequestError {
    match error {
        HttpFetchError::InvalidUrl => HttpRequestError::InvalidUrl,
        HttpFetchError::ForbiddenDestination => HttpRequestError::ForbiddenDestination,
        HttpFetchError::Timeout => HttpRequestError::Timeout,
        _ => HttpRequestError::Unavailable,
    }
}

fn map_transport_error(error: reqwest::Error) -> HttpRequestError {
    if error.is_timeout() {
        HttpRequestError::Timeout
    } else {
        HttpRequestError::Unavailable
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
            mpsc::{self, TryRecvError},
        },
        thread,
        time::{Duration, Instant},
    };

    use rintawa_sdk::{
        contracts::ComponentRef,
        runtime_permissions::RuntimePermission,
        types::{ComponentId, ExtensionInstanceId},
    };

    use super::*;

    fn limits() -> HttpLimits {
        HttpLimits {
            max_body_bytes: 128,
            max_headers: 4,
            max_header_bytes: 128,
            max_response_bytes: 512,
        }
    }

    fn make_request(method: &str, url: &str) -> WitHttpRequest {
        WitHttpRequest {
            method: method.to_owned(),
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
            max_response_bytes: 256,
        }
    }

    fn owner(name: &str) -> ComponentRef {
        ComponentRef::new(ExtensionInstanceId::new(name), ComponentId::new("provider"))
    }

    fn state_with_owner(name: &str) -> WasmHostState {
        let mut state = WasmHostState::new(ComponentId::new("provider"));
        let principal = owner(name);
        state
            .runtime_permissions
            .grant(principal.clone(), RuntimePermission::HttpRequest)
            .expect("test grant should succeed");
        state.execution_owner = Some(principal);
        state.http_request_access_active = true;
        state
    }

    fn insert_fake_request(
        state: &mut WasmHostState,
        principal: ComponentRef,
    ) -> (u64, mpsc::SyncSender<WorkerEvent>, Arc<AtomicBool>) {
        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let handle = state
            .allocate_http_request_handle(principal, receiver, Arc::clone(&cancelled), None)
            .expect("test handle allocation should succeed");
        (handle, sender, cancelled)
    }

    #[test]
    fn test_should_accept_bounded_https_get_and_post_with_auth_header() {
        for method in ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"] {
            let mut request = make_request(method, "https://example.com/v1/chat/completions");
            request.headers.push(WitHttpHeader {
                name: "Authorization".to_owned(),
                value: "Bearer test-credential-placeholder".to_owned(),
            });
            request.body = b"{\"test\":true}".to_vec();
            let validated =
                validate_request(request, limits()).expect("valid request should be accepted");
            assert_eq!(validated.method.as_str(), method);
            assert_eq!(validated.response_limit, 256);
            assert!(validated.headers.contains_key("authorization"));
        }
    }

    #[test]
    fn test_should_reject_non_https_credentials_and_fragment_urls() {
        for url in [
            "http://example.com/path",
            "file:///etc/passwd",
            "https://user@example.com/",
            "https://user:pass@example.com/",
            "https://example.com/path#fragment",
            "https://",
            "https://%ZZ/",
        ] {
            assert!(
                matches!(
                    validate_request(make_request("POST", url), limits()),
                    Err(HttpRequestError::InvalidUrl)
                ),
                "unexpectedly accepted {url}"
            );
        }
    }

    #[test]
    fn test_should_reject_unsupported_or_ambiguous_http_methods() {
        for method in ["TRACE", "CONNECT", "OPTIONS", "post", "", "GET\r\n"] {
            assert!(
                matches!(
                    validate_request(make_request(method, "https://example.com/"), limits()),
                    Err(HttpRequestError::InvalidMethod)
                ),
                "unexpectedly accepted {method}"
            );
        }
    }

    #[test]
    fn test_should_reject_hop_by_hop_header_spoofing() {
        for name in [
            "Host",
            "Connection",
            "Content-Length",
            "Transfer-Encoding",
            "Proxy-Authorization",
            "Proxy-Connection",
            "Upgrade",
            "Trailer",
            "TE",
            "Keep-Alive",
            "Cookie",
        ] {
            let mut request = make_request("POST", "https://example.com/");
            request.headers.push(WitHttpHeader {
                name: name.to_owned(),
                value: "evil".to_owned(),
            });
            assert!(
                matches!(
                    validate_request(request, limits()),
                    Err(HttpRequestError::InvalidHeader)
                ),
                "unexpectedly accepted header {name}"
            );
        }
    }

    #[test]
    fn test_should_reject_header_injection_and_case_insensitive_duplicates() {
        for (name, value) in [
            ("Bad Header", "x"),
            ("X-Token", "a\r\nInjected: true"),
            ("X-Token", "a\0b"),
        ] {
            let mut request = make_request("POST", "https://example.com/");
            request.headers.push(WitHttpHeader {
                name: name.to_owned(),
                value: value.to_owned(),
            });
            assert!(matches!(
                validate_request(request, limits()),
                Err(HttpRequestError::InvalidHeader)
            ));
        }

        let mut request = make_request("GET", "https://example.com/");
        for name in ["X-Token", "x-token"] {
            request.headers.push(WitHttpHeader {
                name: name.to_owned(),
                value: "placeholder".to_owned(),
            });
        }
        assert!(matches!(
            validate_request(request, limits()),
            Err(HttpRequestError::InvalidHeader)
        ));
    }

    #[test]
    fn test_should_enforce_header_body_and_response_quotas() {
        let mut request = make_request("POST", "https://example.com/");
        request.body = vec![0; 129];
        assert!(matches!(
            validate_request(request, limits()),
            Err(HttpRequestError::RequestTooLarge)
        ));

        let mut request = make_request("POST", "https://example.com/");
        request.max_response_bytes = 0;
        assert!(matches!(
            validate_request(request, limits()),
            Err(HttpRequestError::LimitExceeded)
        ));

        let mut request = make_request("POST", "https://example.com/");
        request.max_response_bytes = 1000;
        assert_eq!(
            validate_request(request, limits())
                .expect("response cap should be clamped")
                .response_limit,
            512
        );

        let mut request = make_request("POST", "https://example.com/");
        request.headers = (0..5)
            .map(|i| WitHttpHeader {
                name: format!("X-Header-{i}"),
                value: "v".to_owned(),
            })
            .collect();
        assert!(matches!(
            validate_request(request, limits()),
            Err(HttpRequestError::InvalidHeader)
        ));

        let mut request = make_request("POST", "https://example.com/");
        request.headers.push(WitHttpHeader {
            name: "X-Long".to_owned(),
            value: "x".repeat(129),
        });
        assert!(matches!(
            validate_request(request, limits()),
            Err(HttpRequestError::InvalidHeader)
        ));
    }

    #[test]
    fn test_should_never_start_outbound_network_without_active_grant() {
        let mut state = WasmHostState::new(ComponentId::new("provider"));
        assert!(matches!(
            HttpRequestHost::start(&mut state, make_request("POST", "https://example.com/")),
            Err(HttpRequestError::AccessNotActive)
        ));
        state.http_request_access_active = true;
        state.execution_owner = Some(owner("example"));
        assert!(matches!(
            HttpRequestHost::start(&mut state, make_request("POST", "https://example.com/")),
            Err(HttpRequestError::PermissionDenied)
        ));
        assert!(state.http_requests.is_empty());
    }

    #[test]
    fn test_should_keep_http_access_separate_from_general_host_service_access() {
        let mut state = WasmHostState::new(ComponentId::new("provider"));
        state.instance_id = Some(ExtensionInstanceId::new("example"));
        state.begin_service_execution(None);
        assert!(state.http_request_access_active);
        assert!(!state.host_access_active);
        assert!(!state.network_access_active);
        state.finish_service_execution();
        assert!(!state.http_request_access_active);
    }

    #[test]
    fn test_should_stream_chunk_fragments_without_loss_or_duplication() {
        let mut state = state_with_owner("example");
        let (handle, sender, cancelled) = insert_fake_request(&mut state, owner("example"));
        sender
            .send(WorkerEvent::Head(WitHttpResponseHead {
                status: 200,
                headers: vec![WitHttpHeader {
                    name: "content-type".to_owned(),
                    value: "text/event-stream".to_owned(),
                }],
            }))
            .expect("fake header event should fit");
        sender
            .send(WorkerEvent::Chunk(b"abcdefghij".to_vec()))
            .expect("fake chunk event should fit");
        sender
            .send(WorkerEvent::Completed)
            .expect("fake completion should fit");

        assert!(matches!(HttpRequestHost::poll(&mut state, handle, 3),
            Ok(WitHttpEvent::Response(head)) if head.status == 200));
        for expected in [b"abc".as_slice(), b"def", b"ghi", b"j"] {
            assert!(matches!(HttpRequestHost::poll(&mut state, handle, 3),
                Ok(WitHttpEvent::Data(bytes)) if bytes == expected));
        }
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 3),
            Ok(WitHttpEvent::Completed)
        ));
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 3),
            Ok(WitHttpEvent::Completed)
        ));
        assert!(!cancelled.load(Ordering::Acquire));
        HttpRequestHost::close(&mut state, handle).expect("close should succeed");
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn test_should_keep_pending_and_errors_distinct_from_success() {
        let mut state = state_with_owner("example");
        let (handle, sender, _cancelled) = insert_fake_request(&mut state, owner("example"));
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Pending)
        ));
        sender
            .send(WorkerEvent::Failed(HttpRequestError::Timeout))
            .expect("fake failure event should fit");
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Failed(HttpRequestError::Timeout))
        ));
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Failed(HttpRequestError::Timeout))
        ));
    }

    #[test]
    fn test_should_reject_other_owner_and_unknown_http_handles() {
        let mut state = state_with_owner("first");
        let (handle, _sender, cancelled) = insert_fake_request(&mut state, owner("first"));
        let second = owner("second");
        state
            .runtime_permissions
            .grant(second.clone(), RuntimePermission::HttpRequest)
            .expect("second grant should succeed");
        state.execution_owner = Some(second);
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Err(HttpRequestError::UnknownHandle)
        ));
        assert!(matches!(
            HttpRequestHost::close(&mut state, handle),
            Err(HttpRequestError::UnknownHandle)
        ));
        assert!(!cancelled.load(Ordering::Acquire));
        state.execution_owner = Some(owner("first"));
        HttpRequestHost::close(&mut state, handle).expect("owner close should succeed");
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Err(HttpRequestError::UnknownHandle)
        ));
    }

    #[test]
    fn test_should_enforce_per_component_handle_quota() {
        let mut state = state_with_owner("first");
        state.max_http_request_handles = 1;
        let (_handle, _sender, _cancelled) = insert_fake_request(&mut state, owner("first"));
        assert!(matches!(
            HttpRequestHost::start(&mut state, make_request("GET", "https://example.com/")),
            Err(HttpRequestError::LimitExceeded)
        ));
        assert_eq!(state.http_requests.len(), 1);
    }

    #[test]
    fn test_should_cancel_workers_when_owner_resources_are_revoked() {
        let mut state = state_with_owner("first");
        let (handle, _sender, cancelled) = insert_fake_request(&mut state, owner("first"));
        state.revoke_runtime_resources_for_owner(&owner("first"));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(!state.http_requests.contains_key(&handle));
    }

    #[test]
    fn test_should_wake_bounded_worker_on_cancellation_during_backpressure() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let cancelled = Arc::new(AtomicBool::new(false));
        sender
            .send(WorkerEvent::Chunk(vec![1]))
            .expect("channel has room");
        let worker_cancelled = Arc::clone(&cancelled);
        let worker = thread::spawn(move || {
            send_event(
                &sender,
                &worker_cancelled,
                WorkerEvent::Chunk(vec![2]),
                Instant::now() + Duration::from_secs(1),
            )
        });
        thread::sleep(Duration::from_millis(20));
        cancelled.store(true, Ordering::Release);
        assert!(
            !worker
                .join()
                .expect("worker should finish after cancellation")
        );
        assert!(matches!(receiver.try_recv(), Ok(WorkerEvent::Chunk(bytes)) if bytes == [1]));
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Empty | TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn test_should_stop_stalled_http_operation_after_its_deadline() {
        let mut state = state_with_owner("example");
        let (handle, _sender, cancelled) = insert_fake_request(&mut state, owner("example"));
        let request = state
            .http_requests
            .get_mut(&handle)
            .expect("test request should be present");
        request.deadline = Instant::now() - Duration::from_millis(1);
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 64),
            Ok(WitHttpEvent::Failed(HttpRequestError::Timeout))
        ));
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn test_should_fail_full_channel_at_deadline_without_leaking_thread() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        sender
            .send(WorkerEvent::Chunk(vec![1]))
            .expect("channel has room");
        let cancelled = AtomicBool::new(false);
        let start = Instant::now();
        assert!(!send_event(
            &sender,
            &cancelled,
            WorkerEvent::Chunk(vec![2]),
            start + Duration::from_millis(20),
        ));
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn test_should_reject_private_https_addresses_before_network_request() {
        for url in [
            "https://127.0.0.1/",
            "https://10.0.0.1/",
            "https://169.254.169.254/",
            "https://[::1]/",
            "https://[fc00::1]/",
        ] {
            let parsed = Url::parse(url).expect("test URL is well formed");
            assert!(
                matches!(
                    super::super::network::build_bounded_https_client(
                        &parsed,
                        Duration::from_millis(50)
                    ),
                    Err(HttpFetchError::ForbiddenDestination)
                ),
                "unexpectedly accepted private destination {url}"
            );
        }
    }

    #[test]
    fn test_should_report_worker_destination_policy_failure_as_terminal_error() {
        let validated = validate_request(
            make_request("POST", "https://127.0.0.1/v1/chat/completions"),
            limits(),
        )
        .expect("the policy check is asynchronous");
        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker = thread::spawn({
            let worker_cancelled = Arc::clone(&cancelled);
            move || {
                run_http_worker(
                    validated,
                    Duration::from_millis(50),
                    sender,
                    worker_cancelled,
                )
            }
        });
        worker.join().expect("private URL should fail promptly");
        assert!(matches!(
            receiver.try_recv(),
            Ok(WorkerEvent::Failed(HttpRequestError::ForbiddenDestination))
        ));
    }
}

#[cfg(test)]
mod permit_tests {
    use super::*;

    #[test]
    fn test_should_release_http_worker_slot_on_drop_and_enforce_global_limit() {
        // Isolate the count: other network tests may run HTTP workers in parallel.
        static TEST_WORKERS: AtomicUsize = AtomicUsize::new(0);

        let first = HttpWorkerPermit::acquire_from(&TEST_WORKERS, 1)
            .expect("first permit should be issued");
        assert_eq!(TEST_WORKERS.load(Ordering::Acquire), 1);
        assert!(matches!(
            HttpWorkerPermit::acquire_from(&TEST_WORKERS, 1),
            Err(HttpRequestError::LimitExceeded)
        ));
        drop(first);
        assert_eq!(TEST_WORKERS.load(Ordering::Acquire), 0);
        let second = HttpWorkerPermit::acquire_from(&TEST_WORKERS, 1)
            .expect("released slot must be available again");
        drop(second);
        assert_eq!(TEST_WORKERS.load(Ordering::Acquire), 0);
    }
}

#[cfg(test)]
mod revocation_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use rintawa_sdk::{
        contracts::ComponentRef,
        runtime_permissions::RuntimePermission,
        types::{ComponentId, ExtensionInstanceId},
    };

    use super::*;

    #[test]
    fn test_should_cancel_existing_worker_when_http_permission_is_revoked() {
        let mut state = WasmHostState::new(ComponentId::new("provider"));
        let owner = ComponentRef::new(
            ExtensionInstanceId::new("provider.test"),
            ComponentId::new("provider"),
        );
        state
            .runtime_permissions
            .grant(owner.clone(), RuntimePermission::HttpRequest)
            .expect("test grant should succeed");
        state.execution_owner = Some(owner.clone());
        state.http_request_access_active = true;
        let (_sender, receiver) = mpsc::sync_channel(1);
        let cancellation = Arc::new(AtomicBool::new(false));
        let handle = state
            .allocate_http_request_handle(owner.clone(), receiver, Arc::clone(&cancellation), None)
            .expect("test allocation should succeed");
        state.runtime_permissions.revoke_component(&owner);

        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Err(HttpRequestError::PermissionDenied)
        ));
        assert!(!state.http_requests.contains_key(&handle));
        assert!(cancellation.load(Ordering::Acquire));
    }
}

#[cfg(test)]
mod loopback_http_integration_tests {
    use super::*;
    use rintawa_sdk::{
        contracts::ComponentRef,
        runtime_permissions::RuntimePermission,
        types::{ComponentId, ExtensionInstanceId},
    };
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
        time::{Duration, Instant},
    };

    fn owner(name: &str) -> ComponentRef {
        ComponentRef::new(ExtensionInstanceId::new(name), ComponentId::new("provider"))
    }

    fn state(name: &str) -> WasmHostState {
        let mut state = WasmHostState::new(ComponentId::new("provider"));
        let principal = owner(name);
        state
            .runtime_permissions
            .grant(principal.clone(), RuntimePermission::HttpRequest)
            .expect("test permission should be granted");
        state.execution_owner = Some(principal);
        state.http_request_access_active = true;
        state
    }

    fn make_request(url: &str, method: &str, body: &[u8]) -> WitHttpRequest {
        WitHttpRequest {
            method: method.to_owned(),
            url: url.to_owned(),
            headers: vec![WitHttpHeader {
                name: "Content-Type".to_owned(),
                value: "application/json".to_owned(),
            }],
            body: body.to_vec(),
            max_response_bytes: 1024,
        }
    }

    fn read_http_request(stream: &mut TcpStream) -> Vec<u8> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("test read timeout should be configured");
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        let boundary = loop {
            let n = stream
                .read(&mut buffer)
                .expect("test request must be readable");
            assert_ne!(n, 0, "request ended without HTTP headers");
            bytes.extend_from_slice(&buffer[..n]);
            if let Some(i) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                break i + 4;
            }
        };
        let head = String::from_utf8_lossy(&bytes[..boundary]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|line| {
                line.strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while bytes.len() - boundary < length {
            let n = stream
                .read(&mut buffer)
                .expect("test body must be readable");
            assert_ne!(n, 0, "test request body truncated");
            bytes.extend_from_slice(&buffer[..n]);
        }
        bytes
    }

    fn collect(state: &mut WasmHostState, handle: u64) -> (u16, Vec<u8>) {
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut status = None;
        let mut body = Vec::new();
        loop {
            assert!(Instant::now() < deadline, "HTTP operation stalled");
            match HttpRequestHost::poll(state, handle, 3).expect("HTTP poll should work") {
                WitHttpEvent::Pending => thread::sleep(Duration::from_millis(1)),
                WitHttpEvent::Response(head) => {
                    assert!(status.replace(head.status).is_none(), "duplicate headers");
                }
                WitHttpEvent::Data(chunk) => body.extend(chunk),
                WitHttpEvent::Completed => {
                    return (status.expect("HTTP status must precede completion"), body);
                }
                WitHttpEvent::Failed(error) => panic!("unexpected transport failure: {error:?}"),
            }
        }
    }

    fn respond(status: &str, body: &'static [u8], split: bool) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("test listener should bind");
        let origin = format!(
            "http://127.0.0.1:{}",
            listener.local_addr().expect("test listener address").port()
        );
        let status = status.to_owned();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("test server should accept");
            let received = read_http_request(&mut stream);
            assert!(received.starts_with(b"POST /v1/test HTTP/1.1"));
            assert!(received.ends_with(b"{\"hello\":true}"));
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .expect("test response head should write");
            if split {
                stream
                    .write_all(&body[..3])
                    .expect("first chunk should send");
                thread::sleep(Duration::from_millis(20));
                stream
                    .write_all(&body[3..])
                    .expect("second chunk should send");
            } else {
                stream.write_all(body).expect("response should send");
            }
        });
        (origin, server)
    }

    #[test]
    fn test_should_round_trip_real_loopback_http_post_and_chunked_polls() {
        let (origin, server) = respond("200 OK", b"hello world", true);
        let mut state = state("first");
        state
            .runtime_permissions
            .approve_http_loopback_origin(owner("first"), &origin)
            .expect("exact host approval should succeed");
        let started = Instant::now();
        let handle = HttpRequestHost::start(
            &mut state,
            make_request(&format!("{origin}/v1/test"), "POST", b"{\"hello\":true}"),
        )
        .expect("authorized request should start");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "start must be nonblocking"
        );
        assert_eq!(collect(&mut state, handle), (200, b"hello world".to_vec()));
        HttpRequestHost::close(&mut state, handle).expect("close should succeed");
        server.join().expect("test server should finish");
    }

    #[test]
    fn test_should_deliver_real_http_429_as_response_not_transport_error() {
        let (origin, server) = respond("429 Too Many Requests", b"busy", false);
        let mut state = state("first");
        state
            .runtime_permissions
            .approve_http_loopback_origin(owner("first"), &origin)
            .expect("exact host approval should succeed");
        let handle = HttpRequestHost::start(
            &mut state,
            make_request(&format!("{origin}/v1/test"), "POST", b"{\"hello\":true}"),
        )
        .expect("authorized request should start");
        assert_eq!(collect(&mut state, handle), (429, b"busy".to_vec()));
        HttpRequestHost::close(&mut state, handle).expect("close should succeed");
        server.join().expect("test server should finish");
    }

    #[test]
    fn test_should_reject_ungranted_local_requests_before_allocating_owner_state() {
        let mut state = state("first");
        let next_handle = state.next_http_request_handle;
        assert!(state.http_requests.is_empty());
        assert!(matches!(
            HttpRequestHost::start(
                &mut state,
                make_request("http://127.0.0.1:11434/api", "POST", b"{}")
            ),
            Err(HttpRequestError::ForbiddenDestination)
        ));
        // Unlike the global worker count, this owner's state cannot change
        // because another test ran a valid request on a different thread.
        assert_eq!(state.next_http_request_handle, next_handle);
        assert!(state.http_requests.is_empty());
    }

    #[test]
    fn test_should_not_inherit_approval_across_ports_or_components() {
        let mut state = state("first");
        state
            .runtime_permissions
            .approve_http_loopback_origin(owner("first"), "http://127.0.0.1:11434")
            .expect("host approval should succeed");
        assert!(matches!(
            HttpRequestHost::start(
                &mut state,
                make_request("http://127.0.0.1:11435/api", "POST", b"{}")
            ),
            Err(HttpRequestError::ForbiddenDestination)
        ));
        let second = owner("second");
        state
            .runtime_permissions
            .grant(second.clone(), RuntimePermission::HttpRequest)
            .expect("second grant should succeed");
        state.execution_owner = Some(second);
        assert!(matches!(
            HttpRequestHost::start(
                &mut state,
                make_request("http://127.0.0.1:11434/api", "POST", b"{}")
            ),
            Err(HttpRequestError::ForbiddenDestination)
        ));
    }

    #[test]
    fn test_should_reject_lan_metadata_and_public_cleartext_even_if_local_is_granted() {
        let mut state = state("first");
        state
            .runtime_permissions
            .approve_http_loopback_origin(owner("first"), "http://127.0.0.1:11434")
            .expect("host approval should succeed");
        for endpoint in [
            "http://10.0.0.1:11434/api",
            "http://192.168.1.10:11434/api",
            "http://169.254.169.254/latest/meta-data",
            "http://example.com:11434/api",
        ] {
            assert!(
                matches!(
                    HttpRequestHost::start(&mut state, make_request(endpoint, "GET", b"")),
                    Err(HttpRequestError::InvalidUrl | HttpRequestError::ForbiddenDestination)
                ),
                "unexpectedly accepted: {endpoint}"
            );
        }
    }
}

#[cfg(test)]
mod http_origin_revocation_tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    use rintawa_sdk::{
        contracts::ComponentRef,
        runtime_permissions::RuntimePermission,
        types::{ComponentId, ExtensionInstanceId},
    };

    use super::*;

    #[test]
    fn test_should_not_expose_queued_bytes_after_exact_origin_grant_is_revoked() {
        let principal = ComponentRef::new(
            ExtensionInstanceId::new("test.provider"),
            ComponentId::new("runtime"),
        );
        let origin = "http://127.0.0.1:11434";
        let mut state = WasmHostState::new(ComponentId::new("runtime"));
        state
            .runtime_permissions
            .grant(principal.clone(), RuntimePermission::HttpRequest)
            .expect("test HTTP runtime permission should be granted");
        state
            .runtime_permissions
            .approve_http_loopback_origin(principal.clone(), origin)
            .expect("exact origin should be approved");

        state.execution_owner = Some(principal.clone());
        state.http_request_access_active = true;

        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let endpoint =
            Url::parse("http://127.0.0.1:11434/api/generate").expect("test URL should be valid");
        let handle = state
            .allocate_http_request_handle(
                principal.clone(),
                receiver,
                Arc::clone(&cancelled),
                Some(endpoint),
            )
            .expect("test allocation should succeed");

        sender
            .send(WorkerEvent::Head(WitHttpResponseHead {
                status: 200,
                headers: Vec::new(),
            }))
            .expect("test response headers should be queued");
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Response(head)) if head.status == 200
        ));
        sender
            .send(WorkerEvent::Chunk(b"secret-response".to_vec()))
            .expect("test response bytes should be queued");
        state
            .runtime_permissions
            .revoke_http_loopback_origin(&principal, origin)
            .expect("test grant should be revoked");

        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Failed(HttpRequestError::PermissionDenied))
        ));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(matches!(
            HttpRequestHost::poll(&mut state, handle, 32),
            Ok(WitHttpEvent::Failed(HttpRequestError::PermissionDenied))
        ));
        HttpRequestHost::close(&mut state, handle).expect("owner cleanup should succeed");
    }
}
