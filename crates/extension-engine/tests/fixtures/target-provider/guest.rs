wit_bindgen::generate!({
    path: "wit",
    world: "runtime-provider-plugin",
});

struct Fixture;

impl exports::rintawa::engine::guest::Guest for Fixture {
    fn register() {}

    fn start() {
        let result = rintawa::engine::execution_targets::register_target("test.wasm-target@1");
        assert!(result.is_ok());
    }

    fn stop() {}

    fn on_event(_topic: String, _payload: Vec<u8>) {}

    fn handle_ui_action(_action_json: Vec<u8>) {}

    fn handle_service(_contract: String, _version: u32, _payload: Vec<u8>) -> Vec<u8> {
        Vec::new()
    }
}

impl exports::rintawa::engine::target_provider::Guest for Fixture {
    fn load_component(
        target: String,
        descriptor: exports::rintawa::engine::target_provider::ComponentDescriptor,
        source: &rintawa::engine::execution_targets::ArtifactSource,
    ) -> Result<u64, exports::rintawa::engine::target_provider::Error> {
        use exports::rintawa::engine::target_provider::Error;

        if target != "test.wasm-target@1" || descriptor.target != target {
            return Err(Error::InvalidDescriptor);
        }
        let path = source
            .resolve_component_entry("payload.bin")
            .map_err(|_| Error::Unavailable)?;
        let bytes = source.read(&path).map_err(|_| Error::Unavailable)?;
        if bytes != b"hosted payload" {
            return Err(Error::InvalidDescriptor);
        }
        Ok(7)
    }

    fn register_component(
        handle: u64,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if handle == 7 {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn start_component(
        handle: u64,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if handle != 7 {
            return Err(exports::rintawa::engine::target_provider::Error::UnknownComponent);
        }

        rintawa::engine::runtime_effects::subscribe_event("test.delegated-event@1")
            .map_err(|_| exports::rintawa::engine::target_provider::Error::Unavailable)?;
        Ok(())
    }

    fn stop_component(handle: u64) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if handle == 7 {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn drop_component(handle: u64) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if handle == 7 {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn handle_event(
        handle: u64,
        topic: String,
        payload: Vec<u8>,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        use exports::rintawa::engine::target_provider::Error;

        if handle != 7 {
            return Err(Error::UnknownComponent);
        }
        if topic != "test.delegated-event@1" || payload != b"delegated payload" {
            return Err(Error::Rejected);
        }
        Ok(())
    }

    fn handle_ui_action(
        handle: u64,
        _action_json: Vec<u8>,
    ) -> Result<(), exports::rintawa::engine::target_provider::Error> {
        if handle == 7 {
            Ok(())
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }

    fn handle_service(
        handle: u64,
        _contract: String,
        _version: u32,
        payload: Vec<u8>,
    ) -> Result<Vec<u8>, exports::rintawa::engine::target_provider::Error> {
        if handle == 7 {
            Ok(payload)
        } else {
            Err(exports::rintawa::engine::target_provider::Error::UnknownComponent)
        }
    }
}

export!(Fixture);
