//! Independent native Zenoh client used to test router-side authentication and ACL.
//! Its own client session is intentionally more powerful than the mediated pipe.

use std::{
    io::{self, BufRead, Write},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use zenoh::{Config, Session, Wait};

fn read_line(input: &mut impl BufRead) -> Result<String> {
    let mut value = String::new();
    if input.read_line(&mut value)? == 0 {
        bail!("Host closed the direct-peer configuration channel");
    }
    Ok(value.trim_end().to_owned())
}

fn replies(session: &Session, name: &str) -> Result<Vec<String>> {
    let replies = session
        .get(name)
        .timeout(Duration::from_millis(450))
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    let mut values = Vec::new();
    while let Ok(reply) = replies.recv() {
        if let Ok(sample) = reply.result() {
            values.push(std::str::from_utf8(&sample.payload().to_bytes())?.to_owned());
        }
    }
    Ok(values)
}

fn main() -> Result<()> {
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let endpoint = read_line(&mut input)?;
    let username = read_line(&mut input)?;
    let password = read_line(&mut input)?;
    let own_key = read_line(&mut input)?;
    let foreign_key = read_line(&mut input)?;
    let mut output = io::stdout().lock();

    // Fixed-format test credentials are delivered over a Host-owned pipe,
    // not by command-line arguments or the process environment.
    let config = Config::from_json5(&format!(
        r#"{{
            mode: "client",
            listen: {{endpoints: []}},
            connect: {{endpoints: ["{endpoint}"]}},
            scouting: {{multicast: {{enabled: false}}, gossip: {{enabled: false}}}},
            transport: {{auth: {{usrpwd: {{user: "{username}", password: "{password}"}}}}}}
        }}"#
    ))
    .map_err(|error| anyhow!("{error}"))?;
    let session = zenoh::open(config)
        .wait()
        .map_err(|error| anyhow!("{error}"))
        .context("open authenticated direct client session")?;

    // Wait for the router's queryable declaration to be propagated.
    let mut own_values = Vec::new();
    for _ in 0..20 {
        own_values = replies(&session, &own_key)?;
        if !own_values.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    if own_values.len() != 1 || own_values[0] != username {
        bail!("authenticated client cannot read own public contract: {own_values:?}");
    }

    // A direct session cannot rewrite its authenticated identity by choosing
    // another key expression or using a broad wildcard.
    if replies(&session, &foreign_key).is_ok_and(|values| !values.is_empty()) {
        bail!("foreign World data was delivered through direct Zenoh");
    }
    if replies(&session, "rintawa/probe/direct/**")
        .is_ok_and(|values| values.iter().any(|value| value != &username))
    {
        bail!("wildcard query revealed a foreign World value");
    }

    // Zenoh declarations may return success locally even if the router drops
    // them. Keep the attempted foreign registration alive while the Host
    // independently verifies it is unreachable on the other side.
    let wrong_key = foreign_key.clone();
    let attacker = session
        .declare_queryable(foreign_key.as_str())
        .callback(move |query| {
            let _ = query.reply(wrong_key.as_str(), b"ATTACKER").wait();
        })
        .wait()
        .map_err(|error| anyhow!("{error}"))?;

    session
        .put(foreign_key.as_str(), b"FORGED")
        .wait()
        .map_err(|error| anyhow!("{error}"))?;

    writeln!(output, "READY {username}")?;
    output.flush()?;
    let stop = read_line(&mut input)?;
    if stop != "STOP" {
        bail!("unexpected Host control message");
    }
    attacker
        .undeclare()
        .wait()
        .map_err(|error| anyhow!("{error}"))?;
    session.close().wait().map_err(|error| anyhow!("{error}"))?;
    Ok(())
}
