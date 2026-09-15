use rintawa_sdk::{
    contracts::ComponentRef,
    ui::{UiNode, UiNodeId, UiNodeKind, UiSurfaceContribution, UiSurfaceId, UiTextNode},
};
use rintawa_ui_runtime::UiPresentationSurface;
use rintawa_web_runtime::{
    HostToRendererMessage, RendererToHostMessage, WebBundleDescriptor, WebUiActionEvent,
    WebUiPresentationSurface,
};

fn descriptor() -> WebBundleDescriptor {
    WebBundleDescriptor::parse(
        br#"
schema = 1
bridge-protocol-major = 1
entry = "web/index.html"

[ui-layer]
protocol-major = 1
capabilities = ["rintawa.ui.text@1"]
"#,
    )
    .expect("test descriptor should parse")
}

#[test]
fn test_should_preserve_u64_revision_as_decimal_string() {
    let surface = UiPresentationSurface {
        owner: ComponentRef::new("feature", "runtime"),
        contribution: UiSurfaceContribution::new("main", rintawa_sdk::ui::UiPlacementHint::Primary),
        snapshot: rintawa_sdk::ui::UiSurfaceSnapshot {
            surface_id: UiSurfaceId::new("main"),
            revision: u64::MAX,
            root: UiNodeId::new("root"),
            nodes: vec![UiNode::new(
                "root",
                UiNodeKind::Text(UiTextNode {
                    text: String::from("hello"),
                }),
            )],
        },
    };

    let encoded = WebUiPresentationSurface::from(surface);
    assert_eq!(encoded.snapshot.revision, u64::MAX.to_string());
}

#[test]
fn test_should_reject_web_action_revision_above_u64() {
    let event = WebUiActionEvent {
        owner_instance_id: "feature".into(),
        surface_id: "main".into(),
        node_id: "button".into(),
        action_id: "run".into(),
        surface_revision: String::from("18446744073709551616"),
        payload: rintawa_sdk::ui::UiActionPayload::None,
    };
    assert!(rintawa_sdk::ui::UiActionEvent::try_from(event).is_err());
}

#[test]
fn test_should_validate_exact_renderer_capabilities() {
    let hello: RendererToHostMessage = serde_json::from_value(serde_json::json!({
        "type": "hello",
        "protocol_major": 1,
        "portable_ui_protocol_major": 1,
        "capabilities": ["rintawa.ui.text@1"]
    }))
    .expect("hello should deserialize");
    hello
        .validate(&descriptor())
        .expect("matching capabilities should validate");
}

#[test]
fn test_should_serialize_state_message_with_web_revision() {
    let state = HostToRendererMessage::state(Vec::new());
    let value = serde_json::to_value(state).expect("state should serialize");
    assert_eq!(value["type"], "state");
    assert_eq!(value["protocol_major"], 1);
}

#[test]
fn test_should_allow_web_bundle_without_ui_layer_role() {
    let descriptor = WebBundleDescriptor::parse(
        br#"
schema = 1
bridge-protocol-major = 1
entry = "web/index.html"
"#,
    )
    .expect("descriptor without UI role should parse");
    assert!(descriptor.ui_layer.is_none());
    assert!(descriptor.ui_layer_descriptor().is_none());
}
