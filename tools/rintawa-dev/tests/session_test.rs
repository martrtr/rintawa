use std::{
    fs,
    io::{Read, Write},
    net::TcpStream,
};

use rintawa_dev::{DevProject, DevSession};
use rintawa_extension_engine::ExtensionState;
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

fn write_headless_extension(path: &std::path::Path) -> std::io::Result<()> {
    fs::write(
        path.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        path.join("manifest.toml"),
        "id = \"dev.session\"\nname = \"Dev Session\"\nversion = \"0.1.0\"\nsdk = \"^0.0\"\n",
    )?;
    Ok(())
}

#[test]
fn test_should_run_snapshot_through_extension_engine_lifecycle() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_headless_extension(temp.path())?;
    let project = DevProject::open(temp.path())?;
    let session = DevSession::start(
        &project,
        ExtensionInstanceId::new("test-instance"),
        RuntimeScopeId::new("test-scope"),
    )?;

    assert_eq!(session.extension_id().as_str(), "dev.session");
    assert_eq!(session.instance_id().as_str(), "test-instance");
    assert_eq!(session.state(), Some(ExtensionState::Active));
    session.shutdown()?;
    Ok(())
}

fn write_web_extension(path: &std::path::Path) -> std::io::Result<()> {
    fs::create_dir_all(path.join("web"))?;
    fs::write(
        path.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    fs::write(
        path.join("manifest.toml"),
        concat!(
            "id = \"dev.web-session\"\n",
            "name = \"Dev Web Session\"\n",
            "version = \"0.1.0\"\n",
            "sdk = \"^0.0\"\n\n",
            "[[components]]\n",
            "id = \"web\"\n",
            "kind = \"ui\"\n",
            "target = \"rintawa.runtime.web-bundle@1\"\n",
            "entry = \"web-layer.toml\"\n",
            "required = true\n",
        ),
    )?;
    fs::write(
        path.join("web-layer.toml"),
        concat!(
            "schema = 1\n",
            "bridge-protocol-major = 1\n",
            "entry = \"web/index.html\"\n\n",
            "[ui-layer]\n",
            "protocol-major = 1\n",
            "capabilities = [\"rintawa.ui.text@1\"]\n",
        ),
    )?;
    fs::write(path.join("web/index.html"), "<h1>Rintawa Web Test</h1>")?;
    Ok(())
}

#[test]
fn test_should_serve_packaged_web_bundle_through_dev_session() -> anyhow::Result<()> {
    let temp = tempfile::tempdir()?;
    write_web_extension(temp.path())?;
    let project = DevProject::open(temp.path())?;
    let session = DevSession::start(
        &project,
        ExtensionInstanceId::new("web-test-instance"),
        RuntimeScopeId::new("web-test-scope"),
    )?;

    let urls = session.web_urls()?;
    assert_eq!(urls.len(), 1);
    let address = urls[0]
        .strip_prefix("http://")
        .and_then(|url| url.strip_suffix('/'))
        .ok_or_else(|| anyhow::anyhow!("unexpected Web host URL: {}", urls[0]))?;
    let mut stream = TcpStream::connect(address)?;
    stream.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;

    assert!(response.starts_with("HTTP/1.1 200 OK"));
    assert!(response.contains("Rintawa Web Test"));
    session.shutdown()?;
    Ok(())
}
