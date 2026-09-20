//! WASM network and bounded HTTPS host capabilities.

use std::{
    io::{ErrorKind, Read, Write},
    net::{
        IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream,
        ToSocketAddrs, UdpSocket,
    },
    time::Duration,
};

use reqwest::{
    StatusCode,
    blocking::Client as HttpClient,
    header::{CONTENT_TYPE, LOCATION},
    redirect::Policy as RedirectPolicy,
};
use rintawa_sdk::{contracts::ComponentRef, runtime_permissions::RuntimePermission};
use tracing::warn;
use url::{Host as UrlHost, Url};

use crate::runtime::wasm::{
    HttpFetchError, HttpFetchHost, NetworkError, NetworkHost, RuntimePermissionCheck,
    WasmHostState, WitHttpResponse, WitNetworkListener, WitNetworkReadResult,
};

pub(super) enum NetworkHandleKind {
    Listener(TcpListener),
    Stream(TcpStream),
}

pub(super) struct OwnedNetworkHandle {
    pub(super) owner: ComponentRef,
    pub(super) kind: NetworkHandleKind,
}

impl WasmHostState {
    fn allocate_network_handle(
        &mut self,
        owner: ComponentRef,
        kind: NetworkHandleKind,
    ) -> Result<u64, NetworkError> {
        let owned_count = self
            .network_handles
            .values()
            .filter(|resource| resource.owner == owner)
            .count();
        if owned_count >= self.max_network_handles {
            return Err(NetworkError::LimitExceeded);
        }

        let handle = self.next_network_handle;
        self.next_network_handle = self
            .next_network_handle
            .checked_add(1)
            .ok_or(NetworkError::Unavailable)?;
        self.network_handles
            .insert(handle, OwnedNetworkHandle { owner, kind });
        Ok(handle)
    }
}

impl NetworkHost for WasmHostState {
    fn listen_loopback(&mut self, port: u16) -> Result<WitNetworkListener, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::LoopbackListen)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => NetworkError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => NetworkError::Unavailable,
            })?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let listener = TcpListener::bind(address).map_err(|error| {
            warn!(%address, %error, "Failed to bind WASM loopback listener");
            NetworkError::Unavailable
        })?;
        listener.set_nonblocking(true).map_err(|error| {
            warn!(%address, %error, "Failed to make WASM loopback listener nonblocking");
            NetworkError::Unavailable
        })?;
        let bound_port = listener
            .local_addr()
            .map_err(|error| {
                warn!(%address, %error, "Failed to inspect WASM loopback listener address");
                NetworkError::Unavailable
            })?
            .port();
        let handle = self.allocate_network_handle(owner, NetworkHandleKind::Listener(listener))?;
        Ok(WitNetworkListener {
            handle,
            port: bound_port,
        })
    }

    fn connect_loopback(&mut self, port: u16) -> Result<u64, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        if port == 0 {
            return Err(NetworkError::InvalidPort);
        }
        let owner = self
            .runtime_permission_owner(RuntimePermission::LoopbackConnect)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => NetworkError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => NetworkError::Unavailable,
            })?;
        let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port);
        let stream = TcpStream::connect_timeout(
            &address.into(),
            Duration::from_millis(self.loopback_connect_timeout_ms),
        )
        .map_err(|_| NetworkError::Unavailable)?;
        stream
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Unavailable)?;
        self.allocate_network_handle(owner, NetworkHandleKind::Stream(stream))
    }

    fn accept(&mut self, listener: u64) -> Result<Option<u64>, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(NetworkError::PermissionDenied)?;
        if self
            .network_handles
            .values()
            .filter(|resource| resource.owner == owner)
            .count()
            >= self.max_network_handles
        {
            return Err(NetworkError::LimitExceeded);
        }
        let accepted = {
            let Some(resource) = self.network_handles.get(&listener) else {
                return Err(NetworkError::UnknownHandle);
            };
            if resource.owner != owner {
                return Err(NetworkError::UnknownHandle);
            }
            let NetworkHandleKind::Listener(listener) = &resource.kind else {
                return Err(NetworkError::WrongKind);
            };
            match listener.accept() {
                Ok((stream, _)) => Some(stream),
                Err(error) if error.kind() == ErrorKind::WouldBlock => None,
                Err(_) => return Err(NetworkError::Unavailable),
            }
        };
        let Some(stream) = accepted else {
            return Ok(None);
        };
        stream
            .set_nonblocking(true)
            .map_err(|_| NetworkError::Unavailable)?;
        self.allocate_network_handle(owner, NetworkHandleKind::Stream(stream))
            .map(Some)
    }

    fn read(&mut self, socket: u64, max_bytes: u32) -> Result<WitNetworkReadResult, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(NetworkError::PermissionDenied)?;
        let requested = usize::try_from(max_bytes).map_err(|_| NetworkError::MessageTooLarge)?;
        if requested > self.max_network_io_bytes {
            return Err(NetworkError::MessageTooLarge);
        }
        let Some(resource) = self.network_handles.get_mut(&socket) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        let NetworkHandleKind::Stream(stream) = &mut resource.kind else {
            return Err(NetworkError::WrongKind);
        };
        if requested == 0 {
            return Ok(WitNetworkReadResult {
                data: Vec::new(),
                eof: false,
            });
        }
        let mut data = vec![0_u8; requested];
        match stream.read(&mut data) {
            Ok(0) => Ok(WitNetworkReadResult {
                data: Vec::new(),
                eof: true,
            }),
            Ok(read) => {
                data.truncate(read);
                Ok(WitNetworkReadResult { data, eof: false })
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(NetworkError::WouldBlock),
            Err(_) => Err(NetworkError::Unavailable),
        }
    }

    fn write(&mut self, socket: u64, data: Vec<u8>) -> Result<u32, NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(NetworkError::PermissionDenied)?;
        if data.len() > self.max_network_io_bytes {
            return Err(NetworkError::MessageTooLarge);
        }
        let Some(resource) = self.network_handles.get_mut(&socket) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        let NetworkHandleKind::Stream(stream) = &mut resource.kind else {
            return Err(NetworkError::WrongKind);
        };
        match stream.write(&data) {
            Ok(written) => u32::try_from(written).map_err(|_| NetworkError::MessageTooLarge),
            Err(error) if error.kind() == ErrorKind::WouldBlock => Err(NetworkError::WouldBlock),
            Err(_) => Err(NetworkError::Unavailable),
        }
    }

    fn close(&mut self, handle: u64) -> Result<(), NetworkError> {
        if !self.network_access_active {
            return Err(NetworkError::AccessNotActive);
        }
        let owner = self
            .current_execution_owner()
            .cloned()
            .ok_or(NetworkError::PermissionDenied)?;
        let Some(resource) = self.network_handles.get(&handle) else {
            return Err(NetworkError::UnknownHandle);
        };
        if resource.owner != owner {
            return Err(NetworkError::UnknownHandle);
        }
        self.network_handles.remove(&handle);
        Ok(())
    }
}

impl HttpFetchHost for WasmHostState {
    fn get(&mut self, url: String, max_bytes: u32) -> Result<WitHttpResponse, HttpFetchError> {
        if !self.host_access_active {
            return Err(HttpFetchError::AccessNotActive);
        }
        self.runtime_permission_owner(RuntimePermission::HttpFetch)
            .map_err(|error| match error {
                RuntimePermissionCheck::Denied => HttpFetchError::PermissionDenied,
                RuntimePermissionCheck::Unavailable => HttpFetchError::Unavailable,
            })?;

        let requested_limit = usize::try_from(max_bytes).unwrap_or(usize::MAX);
        let response_limit = requested_limit.min(self.max_http_fetch_bytes);
        let timeout = Duration::from_millis(self.http_fetch_timeout_ms);
        let mut current = Url::parse(&url).map_err(|_| HttpFetchError::InvalidUrl)?;

        for redirect_count in 0..=self.max_http_redirects {
            let client = build_bounded_https_client(&current, timeout)?;
            let response = client
                .get(current.clone())
                .header(reqwest::header::USER_AGENT, "Rintawa/0.0.1")
                .send()
                .map_err(map_http_request_error)?;

            let status = response.status();
            if is_followed_redirect(status) {
                if redirect_count >= self.max_http_redirects {
                    return Err(HttpFetchError::TooManyRedirects);
                }
                let location = response
                    .headers()
                    .get(LOCATION)
                    .ok_or(HttpFetchError::InvalidUrl)?
                    .to_str()
                    .map_err(|_| HttpFetchError::InvalidUrl)?;
                current = current
                    .join(location)
                    .map_err(|_| HttpFetchError::InvalidUrl)?;
                continue;
            }

            if response
                .content_length()
                .is_some_and(|length| length > u64::try_from(response_limit).unwrap_or(u64::MAX))
            {
                return Err(HttpFetchError::ResponseTooLarge);
            }

            let content_type = response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            let final_url = current.to_string();
            let mut body = Vec::with_capacity(
                response_limit
                    .min(response.content_length().unwrap_or(0) as usize)
                    .min(1024 * 1024),
            );
            let read_limit = u64::try_from(response_limit)
                .unwrap_or(u64::MAX)
                .saturating_add(1);
            response
                .take(read_limit)
                .read_to_end(&mut body)
                .map_err(|_| HttpFetchError::Unavailable)?;
            if body.len() > response_limit {
                return Err(HttpFetchError::ResponseTooLarge);
            }

            return Ok(WitHttpResponse {
                status: status.as_u16(),
                final_url,
                content_type,
                body,
            });
        }

        Err(HttpFetchError::TooManyRedirects)
    }
}

pub(super) fn build_bounded_https_client(
    url: &Url,
    timeout: Duration,
) -> Result<HttpClient, HttpFetchError> {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return Err(HttpFetchError::InvalidUrl);
    }
    let host = url.host().ok_or(HttpFetchError::InvalidUrl)?;
    let port = url
        .port_or_known_default()
        .ok_or(HttpFetchError::InvalidUrl)?;

    let mut builder = HttpClient::builder()
        .redirect(RedirectPolicy::none())
        .timeout(timeout)
        .connect_timeout(timeout.min(Duration::from_secs(5)));

    match host {
        UrlHost::Domain(domain) => {
            let addresses = (domain, port)
                .to_socket_addrs()
                .map_err(|_| HttpFetchError::Unavailable)?;
            let allowed = addresses
                .into_iter()
                .find(|address| is_allowed_domain_destination(*address))
                .ok_or(HttpFetchError::ForbiddenDestination)?;
            builder = builder.resolve(domain, allowed);
        }
        UrlHost::Ipv4(address) => {
            if !is_allowed_public_ip(IpAddr::V4(address)) {
                return Err(HttpFetchError::ForbiddenDestination);
            }
        }
        UrlHost::Ipv6(address) => {
            if !is_allowed_public_ip(IpAddr::V6(address)) {
                return Err(HttpFetchError::ForbiddenDestination);
            }
        }
    }

    builder.build().map_err(|_| HttpFetchError::Unavailable)
}

fn is_allowed_domain_destination(address: SocketAddr) -> bool {
    let destination = address.ip();
    let routed_source = if matches!(destination, IpAddr::V4(address) if is_benchmarking_ipv4(address))
    {
        routed_source_ip(address)
    } else {
        None
    };
    is_allowed_domain_destination_with_source(destination, routed_source)
}

pub(super) fn is_allowed_domain_destination_with_source(
    destination: IpAddr,
    routed_source: Option<IpAddr>,
) -> bool {
    if is_allowed_public_ip(destination) {
        return true;
    }

    matches!(
        (destination, routed_source),
        (IpAddr::V4(destination), Some(IpAddr::V4(source)))
            if is_benchmarking_ipv4(destination) && is_benchmarking_ipv4(source)
    )
}

fn routed_source_ip(destination: SocketAddr) -> Option<IpAddr> {
    let bind_address = match destination {
        SocketAddr::V4(_) => SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0)),
        SocketAddr::V6(_) => SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0)),
    };
    let socket = UdpSocket::bind(bind_address).ok()?;
    socket.connect(destination).ok()?;
    socket.local_addr().ok().map(|address| address.ip())
}

fn is_benchmarking_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, _c, _d] = address.octets();
    a == 198 && (b == 18 || b == 19)
}

fn is_followed_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn map_http_request_error(error: reqwest::Error) -> HttpFetchError {
    if error.is_timeout() {
        HttpFetchError::Timeout
    } else {
        HttpFetchError::Unavailable
    }
}

pub(super) fn is_allowed_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => is_allowed_public_ipv4(address),
        IpAddr::V6(address) => is_allowed_public_ipv6(address),
    }
}

fn is_allowed_public_ipv4(address: Ipv4Addr) -> bool {
    let [a, b, c, _d] = address.octets();

    if a == 0
        || a == 10
        || a == 127
        || (a == 100 && (64..=127).contains(&b))
        || (a == 169 && b == 254)
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 192 && b == 0 && c == 0)
        || (a == 192 && b == 0 && c == 2)
        || (a == 198 && (b == 18 || b == 19))
        || (a == 198 && b == 51 && c == 100)
        || (a == 203 && b == 0 && c == 113)
        || a >= 224
    {
        return false;
    }

    true
}

fn is_allowed_public_ipv6(address: Ipv6Addr) -> bool {
    if address.is_unspecified() || address.is_loopback() || address.is_multicast() {
        return false;
    }

    let segments = address.segments();
    if (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xffc0) == 0xfec0
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
    {
        return false;
    }

    if segments[..5] == [0, 0, 0, 0, 0] && segments[5] == 0xffff {
        let octets = address.octets();
        return is_allowed_public_ipv4(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ));
    }

    true
}
