//! Guest fixture for execution-target provider integration tests.

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

}

export!(Fixture);
