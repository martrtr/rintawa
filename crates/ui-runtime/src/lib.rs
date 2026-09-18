//! Renderer-neutral portable UI state, validation, patching, and action routing.

#![forbid(unsafe_code)]
#![warn(missing_docs, rustdoc::broken_intra_doc_links)]

mod runtime;
mod validation;

pub use runtime::{
    OwnedUiLayerDescriptor, OwnedUiSurfaceContribution, QueuedUiAction, UiActionDispatch,
    UiPresentationSurface, UiRuntime,
};
