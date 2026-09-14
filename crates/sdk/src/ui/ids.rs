//! Identifier types and capability names for portable UI.

use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! ui_string_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            /// Creates a new identifier.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Returns the identifier as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }
    };
}

ui_string_id!(UiSurfaceId, "Identifies one portable UI surface.");
ui_string_id!(
    UiNodeId,
    "Identifies one node within a portable UI surface."
);
ui_string_id!(
    UiActionId,
    "Identifies one semantic action exposed by a UI node."
);
ui_string_id!(
    UiCapabilityId,
    "Identifies one presentation capability supported by a UI Layer."
);

/// Portable UI protocol major version implemented by this SDK.
pub const PORTABLE_UI_PROTOCOL_MAJOR: u32 = 1;

/// Capability required for plain text nodes.
pub const UI_CAPABILITY_TEXT: &str = "rintawa.ui.text@1";
/// Capability required for Markdown nodes.
pub const UI_CAPABILITY_MARKDOWN: &str = "rintawa.ui.markdown@1";
/// Capability required for buttons.
pub const UI_CAPABILITY_BUTTON: &str = "rintawa.ui.button@1";
/// Capability required for single-line text input.
pub const UI_CAPABILITY_TEXT_INPUT: &str = "rintawa.ui.input.text@1";
/// Capability required for multiline text input.
pub const UI_CAPABILITY_TEXT_AREA: &str = "rintawa.ui.input.text-area@1";
/// Capability required for horizontal containers.
pub const UI_CAPABILITY_ROW: &str = "rintawa.ui.layout.row@1";
/// Capability required for vertical containers.
pub const UI_CAPABILITY_COLUMN: &str = "rintawa.ui.layout.column@1";
/// Capability required for list containers.
pub const UI_CAPABILITY_LIST: &str = "rintawa.ui.list@1";
