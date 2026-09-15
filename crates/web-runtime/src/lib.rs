//! Host-side protocol for `rintawa.runtime.web-bundle@1` components.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod descriptor;
mod error;
mod wire;

pub use descriptor::{
    WEB_BUNDLE_DESCRIPTOR_MAX_BYTES, WEB_BUNDLE_DESCRIPTOR_SCHEMA, WEB_BUNDLE_TARGET_V1,
    WEB_UI_BRIDGE_PROTOCOL_MAJOR, WebBundleDescriptor, WebUiLayerDescriptor,
};
pub use error::{WebRuntimeError, WebRuntimeResult};
pub use wire::{
    HostToRendererMessage, RendererToHostMessage, WebUiActionEvent, WebUiPresentationSurface,
    WebUiSurfaceSnapshot,
};
