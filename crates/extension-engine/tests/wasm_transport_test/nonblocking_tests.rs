//! D8 imports through a real ordinary WASM callback and exact activation restart.
use super::*;
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
