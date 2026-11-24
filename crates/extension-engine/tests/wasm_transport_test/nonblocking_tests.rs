//! D8 imports through a real ordinary WASM callback and exact activation restart.
use super::*;
use rintawa_sdk::contracts::ComponentPrincipal;
use std::time::{Duration, Instant};
fn call(
    engine: &ExtensionEngine,
    owner: &rintawa_sdk::contracts::ComponentPrincipal,
    command: &str,
) -> Result<Vec<u8>> {
    let replies =
        engine.query_transport_contributions(owner, "fixture/caller", command.as_bytes())?;
    anyhow::ensure!(replies.len() == 1, "expected one fixture");
    Ok(replies[0].clone())
}
fn admitted(
    engine: &ExtensionEngine,
    owner: &rintawa_sdk::contracts::ComponentPrincipal,
    command: &str,
) -> Result<Vec<u8>> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let reply = call(engine, owner, command)?;
        if String::from_utf8_lossy(&reply).contains("Unavailable") {
            anyhow::ensure!(Instant::now() < until, "busy admission did not clear");
            std::thread::sleep(Duration::from_millis(1));
            continue;
        }
        return Ok(reply);
    }
}
fn handle(reply: Vec<u8>) -> Result<u64> {
    let reply = String::from_utf8(reply)?;
    Ok(reply
        .strip_prefix("handle:")
        .ok_or_else(|| anyhow::anyhow!("{reply}"))?
        .parse()?)
}
fn wait_reply(
    engine: &ExtensionEngine,
    owner: &rintawa_sdk::contracts::ComponentPrincipal,
    command: &str,
) -> Result<Vec<u8>> {
    let until = Instant::now() + Duration::from_secs(2);
    loop {
        let reply = call(engine, owner, command)?;
        if reply != b"pending" && !String::from_utf8_lossy(&reply).contains("Unavailable") {
            return Ok(reply);
        }
        anyhow::ensure!(Instant::now() < until, "missing response: {command}");
        std::thread::sleep(Duration::from_millis(1));
    }
}
#[test]
fn test_ordinary_wasm_uses_all_nonblocking_imports_and_rejects_other_world_and_restart_handles()
-> Result<()> {
    let mut engine = ExtensionEngine::new();
    for (id, scope) in [("ordinary", "world-a"), ("foreign", "world-b")] {
        let component = engine
            .wasm_runtime_engine()?
            .load_component_from_bytes(ComponentId::new("runtime"), PROVIDER)?;
        engine.register_extension_instance(
            id.into(),
            scope.into(),
            manifest(id),
            vec![Box::new(component)],
        )?;
        engine.start_extension_instance(&id.into())?;
    }
    let owner = ComponentRef::new("ordinary", "runtime");
    let principal = engine.active_component_principal(&owner)?;
    let foreign = engine.active_component_principal(&ComponentRef::new("foreign", "runtime"))?;
    let declaration = handle(admitted(
        &engine,
        &principal,
        "d8:declare:fixture/ordinary",
    )?)?;
    let outgoing = handle(admitted(
        &engine,
        &principal,
        "d8:begin:fixture/ordinary\nspoofed caller stays payload",
    )?)?;
    let incoming = String::from_utf8(wait_reply(
        &engine,
        &principal,
        &format!("d8:next:{declaration}"),
    )?)?;
    let (header, payload) = incoming.split_once('\n').unwrap();
    let fields: Vec<_> = header.split('|').collect();
    let request: u64 = fields[0].parse()?;
    assert_eq!(&fields[1..3], &["ordinary", "runtime"]);
    assert_eq!(fields[3].parse::<u64>()?, principal.activation);
    assert_eq!(payload, "spoofed caller stays payload");
    assert!(
        String::from_utf8_lossy(&admitted(
            &engine,
            &foreign,
            &format!("d8:next:{declaration}")
        )?)
        .contains("InvalidHandle")
    );
    assert_eq!(
        admitted(&engine, &principal, &format!("d8:reply:{request}\nanswer"))?,
        b"ok"
    );
    assert_eq!(
        wait_reply(&engine, &principal, &format!("d8:poll:{outgoing}"))?,
        b"reply:answer"
    );
    let closed = handle(admitted(
        &engine,
        &principal,
        "d8:begin:fixture/ordinary\nclose receiver",
    )?)?;
    assert_eq!(
        admitted(&engine, &principal, &format!("d8:close:{closed}"))?,
        b"closed"
    );
    engine.stop_extension_instance(&owner.instance_id)?;
    engine.start_extension_instance(&owner.instance_id)?;
    let replacement = engine.active_component_principal(&owner)?;
    assert_ne!(replacement.activation, principal.activation);
    assert!(
        String::from_utf8_lossy(&admitted(
            &engine,
            &replacement,
            &format!("d8:next:{declaration}")
        )?)
        .contains("InvalidHandle")
    );
    let fresh = handle(admitted(
        &engine,
        &replacement,
        "d8:declare:fixture/ordinary",
    )?)?;
    assert_ne!(fresh, declaration);
    assert_eq!(
        admitted(&engine, &replacement, &format!("d8:undeclare:{fresh}"))?,
        b"closed"
    );
    engine.stop_extension_instance(&owner.instance_id)?;
    engine.stop_extension_instance(&"foreign".into())?;
    Ok(())
}

#[test]
fn test_directed_wasm_calls_echo_attested_caller_in_callback_and_polled_providers() -> Result<()> {
    let mut engine = ExtensionEngine::new();
    let scope = RuntimeScopeId::new("world-a");
    let callback = "chat/send-controls/v1";
    let polled = "fixture/directed-polled";
    let selected = ComponentRef::new("selected", "runtime");
    for (id, world, bytes) in [
        ("selected", "world-a", PROVIDER),
        ("consumer", "world-a", CONSUMER),
        ("foreign", "world-b", PROVIDER),
    ] {
        let component = engine
            .wasm_runtime_engine()?
            .load_component_from_bytes(ComponentId::new("runtime"), bytes)?;
        engine.register_extension_instance(
            id.into(),
            world.into(),
            manifest(id),
            vec![Box::new(component)],
        )?;
        engine.start_extension_instance(&id.into())?;
    }
    let caller = engine.active_component_principal(&ComponentRef::new("consumer", "runtime"))?;
    let provider = engine.active_component_principal(&selected)?;
    let foreign = engine.active_component_principal(&ComponentRef::new("foreign", "runtime"))?;
    let identity = |principal: &ComponentPrincipal| {
        format!(
            "{}|{}|{}",
            principal.instance_id, principal.component_id, principal.activation
        )
    };
    let send = |engine: &ExtensionEngine, command: &str| -> Result<String> {
        let replies = engine.query_transport_contributions(
            &caller,
            "fixture/consumer",
            command.as_bytes(),
        )?;
        let [reply] = replies.as_slice() else {
            anyhow::bail!("expected one consumer response")
        };
        Ok(String::from_utf8(reply.clone())?)
    };
    let begin = |engine: &ExtensionEngine, point, target: &ComponentPrincipal, payload| {
        send(
            engine,
            &format!("directed-begin:{point}|{}\n{payload}", identity(target)),
        )
    };
    let begin_admitted =
        |engine: &ExtensionEngine, point, target: &ComponentPrincipal, payload| -> Result<u64> {
            let until = Instant::now() + Duration::from_secs(2);
            loop {
                let reply = begin(engine, point, target, payload)?;
                if reply != "error:Error::Unavailable" {
                    return Ok(reply.parse()?);
                }
                anyhow::ensure!(Instant::now() < until, "directed admission remained busy");
                std::thread::sleep(Duration::from_millis(1));
            }
        };
    let poll = |engine: &ExtensionEngine, h| -> Result<String> {
        let until = Instant::now() + Duration::from_secs(2);
        loop {
            let reply = send(engine, &format!("sourced-poll:{h}"))?;
            if reply != "pending" {
                return Ok(reply);
            }
            anyhow::ensure!(Instant::now() < until, "directed reply remained pending");
            std::thread::sleep(Duration::from_millis(1));
        }
    };

    assert_eq!(
        begin(&engine, callback, &provider, "directed-effect:absent")?,
        "error:Error::NotSelected"
    );
    engine.set_authoritative_transport_provider_in_scope(&scope, callback, &selected)?;
    engine.set_authoritative_transport_provider_in_scope(&scope, polled, &selected)?;
    assert_eq!(
        begin(&engine, callback, &foreign, "directed-effect:foreign")?,
        "error:Error::NotSelected"
    );
    assert_eq!(call(&engine, &provider, "directed-count")?, b"0");
    let outgoing = begin_admitted(&engine, callback, &provider, "directed-effect:original")?;
    assert_eq!(
        poll(&engine, outgoing)?,
        format!("{}|{}|original", identity(&provider), identity(&caller))
    );
    assert_eq!(call(&engine, &provider, "directed-count")?, b"1");

    let declaration = handle(admitted(
        &engine,
        &provider,
        &format!("d8:declare:{polled}"),
    )?)?;
    let outgoing = begin_admitted(&engine, polled, &provider, "private-context")?;
    let incoming = String::from_utf8(wait_reply(
        &engine,
        &provider,
        &format!("d8:next:{declaration}"),
    )?)?;
    let (header, payload) = incoming.split_once('\n').unwrap();
    let (request, attested) = header.split_once('|').unwrap();
    assert_eq!(payload, "private-context");
    assert_eq!(attested, identity(&caller));
    assert_eq!(
        admitted(
            &engine,
            &provider,
            &format!("d8:reply:{request}\n{attested}")
        )?,
        b"ok"
    );
    assert_eq!(
        poll(&engine, outgoing)?,
        format!("{}|{}", identity(&provider), identity(&caller))
    );

    engine.stop_extension_instance(&selected.instance_id)?;
    engine.start_extension_instance(&selected.instance_id)?;
    let replacement = engine.active_component_principal(&selected)?;
    assert_ne!(replacement.activation, provider.activation);
    assert_eq!(
        begin(&engine, callback, &provider, "directed-effect:stale")?,
        "error:Error::Unavailable"
    );
    assert_eq!(call(&engine, &replacement, "directed-count")?, b"0");
    let outgoing = begin_admitted(
        &engine,
        callback,
        &replacement,
        "directed-effect:replacement",
    )?;
    assert_eq!(
        poll(&engine, outgoing)?,
        format!(
            "{}|{}|replacement",
            identity(&replacement),
            identity(&caller)
        )
    );
    assert_eq!(call(&engine, &replacement, "directed-count")?, b"1");
    Ok(())
}
