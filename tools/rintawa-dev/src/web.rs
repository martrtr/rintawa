use std::{
    collections::BTreeMap,
    net::TcpListener,
    sync::{Arc, Mutex, mpsc},
    thread,
};

use axum::{
    Router,
    body::Body,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::{StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::{SinkExt, StreamExt};
use rintawa_extension_engine::{
    ExtensionEngine, RtwComponentHost, RtwComponentHostError, RtwComponentHostResult,
    RtwComponentSource,
};
use rintawa_sdk::{
    context::{ComponentContext, RegistrationContext},
    contracts::{
        ComponentRef, ContractDefinition, ContractKey, ContractProvider, ContractResolutionPolicy,
        ContractVersion,
    },
    errors::{ExtensionError, ExtensionResult},
    manifest::ComponentDescriptor,
    traits::Component,
    types::ComponentId,
    ui::UiActionEvent,
};
use rintawa_web_runtime::{
    HostToRendererMessage, RendererToHostMessage, WEB_BUNDLE_TARGET_V1, WebBundleDescriptor,
};
use tokio::sync::{oneshot, watch};

use crate::{DevError, DevResult};

const UI_LAYER_CONTRACT_ID: &str = "rintawa.ui.layer";
const UI_LAYER_CONTRACT_VERSION: u32 = 1;
const MAX_DEV_WEB_BUNDLE_BYTES: usize = 64 * 1024 * 1024;
const WEBSOCKET_PATH: &str = "/__rintawa/ws";

struct QueuedWebAction {
    layer_owner: ComponentRef,
    event: UiActionEvent,
    response: oneshot::Sender<Result<(), String>>,
}

#[derive(Clone)]
struct WebAsset {
    bytes: Arc<[u8]>,
    content_type: &'static str,
}

struct DevWebServer {
    url: String,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl DevWebServer {
    fn stop(mut self) -> DevResult<()> {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| DevError::WebHost(String::from("Web host thread panicked")))?;
        }
        Ok(())
    }
}

struct DevWebComponentState {
    component_id: ComponentId,
    descriptor: WebBundleDescriptor,
    entry_path: String,
    assets: Arc<BTreeMap<String, WebAsset>>,
    action_sender: mpsc::Sender<QueuedWebAction>,
    owner: Mutex<Option<ComponentRef>>,
    server: Mutex<Option<DevWebServer>>,
    state_sender: watch::Sender<Option<String>>,
    last_state: Mutex<Option<String>>,
}

impl DevWebComponentState {
    fn owner(&self) -> DevResult<Option<ComponentRef>> {
        self.owner
            .lock()
            .map(|owner| owner.clone())
            .map_err(|_| DevError::WebHostUnavailable)
    }

    fn url(&self) -> DevResult<Option<String>> {
        self.server
            .lock()
            .map(|server| server.as_ref().map(|server| server.url.clone()))
            .map_err(|_| DevError::WebHostUnavailable)
    }

    fn start(&self, owner: ComponentRef) -> DevResult<()> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let address = listener.local_addr()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(DevError::Io)?;
        let (shutdown_sender, shutdown_receiver) = oneshot::channel();
        let state = Arc::new(WebServerState {
            descriptor: self.descriptor.clone(),
            entry_path: self.entry_path.clone(),
            assets: self.assets.clone(),
            owner: owner.clone(),
            action_sender: self.action_sender.clone(),
            state_receiver: self.state_sender.subscribe(),
        });
        let router = Router::new()
            .route(WEBSOCKET_PATH, get(websocket_upgrade))
            .fallback(get(serve_asset))
            .with_state(state);
        let thread = thread::Builder::new()
            .name(format!("rintawa-web-{}", self.component_id))
            .spawn(move || {
                runtime.block_on(async move {
                    let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
                        return;
                    };
                    let server = axum::serve(listener, router).with_graceful_shutdown(async {
                        let _ = shutdown_receiver.await;
                    });
                    let _ = server.await;
                });
            })?;

        let server = DevWebServer {
            url: format!("http://{address}/"),
            shutdown: Some(shutdown_sender),
            thread: Some(thread),
        };
        *self
            .owner
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)? = Some(owner);
        *self
            .server
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)? = Some(server);
        Ok(())
    }

    fn stop(&self) -> DevResult<()> {
        let server = self
            .server
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)?
            .take();
        *self
            .owner
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)? = None;
        if let Some(server) = server {
            server.stop()?;
        }
        Ok(())
    }

    fn publish(&self, message: HostToRendererMessage) -> DevResult<()> {
        let encoded = serde_json::to_string(&message).map_err(|error| {
            DevError::WebHost(format!("failed to encode Web UI state: {error}"))
        })?;
        let mut last_state = self
            .last_state
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)?;
        if last_state.as_ref() == Some(&encoded) {
            return Ok(());
        }
        *last_state = Some(encoded.clone());
        self.state_sender.send_replace(Some(encoded));
        Ok(())
    }
}

struct DevWebComponent {
    state: Arc<DevWebComponentState>,
}

impl Component for DevWebComponent {
    fn id(&self) -> &ComponentId {
        &self.state.component_id
    }

    fn register(&mut self, ctx: &mut dyn RegistrationContext) -> ExtensionResult<()> {
        if self.state.descriptor.ui_layer.is_some() {
            let contract = ContractKey::new(
                UI_LAYER_CONTRACT_ID,
                ContractVersion::new(UI_LAYER_CONTRACT_VERSION),
            );
            ctx.define_contract(ContractDefinition::new(
                contract.clone(),
                ContractResolutionPolicy::Single,
            ))?;
            ctx.provide_contract(ContractProvider::new(contract))?;
        }
        Ok(())
    }

    fn start(&mut self, ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        let owner = ComponentRef::new(
            ctx.extension_instance_id().clone(),
            ctx.component_id().clone(),
        );
        self.state
            .start(owner)
            .map_err(|error| ExtensionError::Message(error.to_string()))
    }

    fn stop(&mut self, _ctx: &mut dyn ComponentContext) -> ExtensionResult<()> {
        self.state
            .stop()
            .map_err(|error| ExtensionError::Message(error.to_string()))
    }
}

impl Drop for DevWebComponent {
    fn drop(&mut self) {
        let _ = self.state.stop();
    }
}

/// Development host for packaged Web bundle components.
pub struct DevWebComponentHost {
    components: Mutex<Vec<Arc<DevWebComponentState>>>,
    action_sender: mpsc::Sender<QueuedWebAction>,
    action_receiver: Mutex<mpsc::Receiver<QueuedWebAction>>,
}

impl DevWebComponentHost {
    /// Creates an empty Web component host.
    pub fn new() -> Self {
        let (action_sender, action_receiver) = mpsc::channel();
        Self {
            components: Mutex::new(Vec::new()),
            action_sender,
            action_receiver: Mutex::new(action_receiver),
        }
    }

    pub(crate) fn attach_layers(&self, engine: &mut ExtensionEngine) -> DevResult<()> {
        for component in self.components()? {
            if let Some(owner) = component.owner()?
                && let Some(descriptor) = component.descriptor.ui_layer_descriptor()
            {
                engine.attach_ui_layer(owner, descriptor)?;
            }
        }
        Ok(())
    }

    pub(crate) fn pump(&self, engine: &mut ExtensionEngine) -> DevResult<()> {
        let actions: Vec<_> = self
            .action_receiver
            .lock()
            .map_err(|_| DevError::WebHostUnavailable)?
            .try_iter()
            .collect();
        for action in actions {
            let result = engine
                .dispatch_ui_action(&action.layer_owner, action.event)
                .map_err(|error| error.to_string());
            let _ = action.response.send(result);
        }
        self.publish_state(engine)
    }

    pub(crate) fn urls(&self) -> DevResult<Vec<String>> {
        self.components()?
            .into_iter()
            .filter_map(|component| match component.url() {
                Ok(Some(url)) => Some(Ok(url)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }

    fn components(&self) -> DevResult<Vec<Arc<DevWebComponentState>>> {
        self.components
            .lock()
            .map(|components| components.clone())
            .map_err(|_| DevError::WebHostUnavailable)
    }

    fn publish_state(&self, engine: &ExtensionEngine) -> DevResult<()> {
        for component in self.components()? {
            let Some(owner) = component.owner()? else {
                continue;
            };
            let surfaces = engine.portable_ui_surfaces_for_layer(&owner)?;
            component.publish(HostToRendererMessage::state(surfaces))?;
        }
        Ok(())
    }
}

impl Default for DevWebComponentHost {
    fn default() -> Self {
        Self::new()
    }
}

impl RtwComponentHost for DevWebComponentHost {
    fn target(&self) -> &str {
        WEB_BUNDLE_TARGET_V1
    }

    fn load_component(
        &self,
        source: &mut RtwComponentSource<'_>,
        component: &ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        let descriptor_entry = component.entry.as_deref().ok_or_else(|| {
            RtwComponentHostError::InvalidDescriptor(String::from(
                "Web bundle component requires an entry descriptor",
            ))
        })?;
        let descriptor_path = source.resolve_component_entry(descriptor_entry)?;
        let descriptor_bytes = source.read(&descriptor_path)?;
        let descriptor = WebBundleDescriptor::parse(&descriptor_bytes)
            .map_err(|error| RtwComponentHostError::InvalidDescriptor(error.to_string()))?;
        let web_entry = source.resolve_relative_to(&descriptor_path, descriptor.entry.as_str())?;
        let (root, entry_path) = web_entry.as_str().rsplit_once('/').ok_or_else(|| {
            RtwComponentHostError::InvalidDescriptor(String::from(
                "Web bundle entry must be inside a dedicated artifact directory",
            ))
        })?;
        let prefix = format!("{root}/");
        let mut assets = BTreeMap::new();
        let mut total_bytes = 0_usize;
        for path in source.paths() {
            let Some(relative) = path.as_str().strip_prefix(&prefix) else {
                continue;
            };
            if relative.is_empty() {
                continue;
            }
            let bytes = source.read(&path)?;
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > MAX_DEV_WEB_BUNDLE_BYTES {
                return Err(RtwComponentHostError::Host(format!(
                    "Web bundle exceeds the {MAX_DEV_WEB_BUNDLE_BYTES}-byte development host limit"
                )));
            }
            assets.insert(
                relative.to_string(),
                WebAsset {
                    content_type: content_type(relative),
                    bytes: bytes.into(),
                },
            );
        }
        if !assets.contains_key(entry_path) {
            return Err(RtwComponentHostError::InvalidDescriptor(format!(
                "Web bundle entry `{entry_path}` was not found"
            )));
        }

        let (state_sender, _) = watch::channel(None);
        let state = Arc::new(DevWebComponentState {
            component_id: component.id.clone(),
            descriptor,
            entry_path: entry_path.to_string(),
            assets: Arc::new(assets),
            action_sender: self.action_sender.clone(),
            owner: Mutex::new(None),
            server: Mutex::new(None),
            state_sender,
            last_state: Mutex::new(None),
        });
        self.components
            .lock()
            .map_err(|_| {
                RtwComponentHostError::Host(String::from("Web host state is unavailable"))
            })?
            .push(state.clone());
        Ok(Box::new(DevWebComponent { state }))
    }
}

#[derive(Clone)]
struct WebServerState {
    descriptor: WebBundleDescriptor,
    entry_path: String,
    assets: Arc<BTreeMap<String, WebAsset>>,
    owner: ComponentRef,
    action_sender: mpsc::Sender<QueuedWebAction>,
    state_receiver: watch::Receiver<Option<String>>,
}

async fn websocket_upgrade(
    websocket: WebSocketUpgrade,
    State(state): State<Arc<WebServerState>>,
) -> impl IntoResponse {
    websocket.on_upgrade(move |socket| websocket_session(socket, state))
}

async fn websocket_session(socket: WebSocket, state: Arc<WebServerState>) {
    let (mut sender, mut receiver) = socket.split();
    let Some(Ok(Message::Text(first))) = receiver.next().await else {
        return;
    };
    let hello = match serde_json::from_str::<RendererToHostMessage>(first.as_str()) {
        Ok(message @ RendererToHostMessage::Hello { .. }) => message,
        Ok(_) => {
            let _ =
                send_web_error(&mut sender, "handshake-required", "expected hello message").await;
            return;
        }
        Err(error) => {
            let _ = send_web_error(&mut sender, "invalid-message", &error.to_string()).await;
            return;
        }
    };
    if let Err(error) = hello.validate(&state.descriptor) {
        let _ = send_web_error(&mut sender, "incompatible-renderer", &error.to_string()).await;
        return;
    }

    let mut state_receiver = state.state_receiver.clone();
    let initial_state = state_receiver.borrow_and_update().clone();
    if let Some(message) = initial_state
        && sender.send(Message::Text(message.into())).await.is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            incoming = receiver.next() => {
                let Some(Ok(message)) = incoming else { break; };
                if !handle_renderer_message(message, &state, &mut sender).await {
                    break;
                }
            }
            changed = state_receiver.changed() => {
                if changed.is_err() { break; }
                let message = state_receiver.borrow().clone();
                if let Some(message) = message
                    && sender.send(Message::Text(message.into())).await.is_err()
                {
                    break;
                }
            }
        }
    }
}

async fn handle_renderer_message(
    message: Message,
    state: &WebServerState,
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
) -> bool {
    let Message::Text(text) = message else {
        return true;
    };
    let parsed = match serde_json::from_str::<RendererToHostMessage>(text.as_str()) {
        Ok(message) => message,
        Err(error) => {
            let _ = send_web_error(sender, "invalid-message", &error.to_string()).await;
            return true;
        }
    };
    if let Err(error) = parsed.validate(&state.descriptor) {
        let _ = send_web_error(sender, "invalid-message", &error.to_string()).await;
        return true;
    }
    let RendererToHostMessage::Action { event, .. } = parsed else {
        return true;
    };
    let event = match UiActionEvent::try_from(event) {
        Ok(event) => event,
        Err(error) => {
            let _ = send_web_error(sender, "invalid-action", &error.to_string()).await;
            return true;
        }
    };
    let (response_sender, response_receiver) = oneshot::channel();
    if state
        .action_sender
        .send(QueuedWebAction {
            layer_owner: state.owner.clone(),
            event,
            response: response_sender,
        })
        .is_err()
    {
        let _ = send_web_error(sender, "host-unavailable", "Rintawa host is unavailable").await;
        return false;
    }

    match response_receiver.await {
        Ok(Ok(())) => true,
        Ok(Err(reason)) => {
            let _ = send_web_error(sender, "action-rejected", &reason).await;
            true
        }
        Err(_) => {
            let _ = send_web_error(
                sender,
                "host-unavailable",
                "Rintawa host did not process the action",
            )
            .await;
            false
        }
    }
}

async fn send_web_error(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    code: &str,
    message: &str,
) -> Result<(), axum::Error> {
    let encoded = match serde_json::to_string(&HostToRendererMessage::error(code, message)) {
        Ok(encoded) => encoded,
        Err(_) => return Ok(()),
    };
    sender.send(Message::Text(encoded.into())).await
}

async fn serve_asset(State(state): State<Arc<WebServerState>>, uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let path = if requested.is_empty() {
        state.entry_path.as_str()
    } else {
        requested
    };
    let Some(asset) = state.assets.get(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type)
        .header(header::CACHE_CONTROL, "no-store")
        .body(Body::from(asset.bytes.to_vec()))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("wasm") => "application/wasm",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::content_type;

    #[test]
    fn test_should_serve_webassembly_with_standard_content_type() {
        assert_eq!(content_type("runtime.wasm"), "application/wasm");
    }
}
