# Zenoh native/WASM transport feasibility probe

**Status: isolated experiment, not a Rintawa runtime feature.** This lives outside
the workspace and changes no production SDK, WIT, Engine, Host or World API.

## Run

Run `./run.sh` from this directory, or any other directory. The script builds
the real Rust/WIT guest for `wasm32-unknown-unknown`, wraps it into a
component with `wit-component`, compiles the native binaries and runs eight
transport tests with pinned Cargo.lock files. Linux, Bubblewrap (`bwrap`),
Rust stable and the wasm32 Rust target are required. The direct-ACL test
uses an ephemeral loopback TCP port.
The default output is in this directory's ignored target directory. Override
the `CARGO_TARGET_DIR` environment variable to share a local target directory.

## Confirmed boundaries in the experiment

- Zenoh 1.10.2, one Host-owned isolated Peer session with discovery, listen
  and connect disabled. Native extensions do not receive that session.
- A separate Linux native executable is launched through Bubblewrap,
  with a private network namespace, read-only system mounts, and inherited
  stdin/stdout pipes. Host gives each process a distinct trusted fixture
  principal and validates short local requests before performing Zenoh
  operations. The two processes see distinct responses to the same public
  name; foreign full keys and wildcards are rejected. A single three-second
  Host deadline now bounds the entire exchange and process exit; peers that
  hang before a request or after a successful exchange are killed and reaped,
  while an independent World answers during and after the hang. This is
  deliberately **not** a proof of raw Zenoh ACL or production supervision.
- Wasmtime 29 with one WASM guest instance in each of two simulated World scopes.
  WIT provides no raw socket, Zenoh session, namespace prefix, or user-controlled
  authority/identity fields to the guest.
- Host maps short, validated local names to its own trusted fixture principal
  and World scope, rejecting traversal and wildcards. The guest attempts two
  forbidden registrations/publications before starting normally.
- Native test code queries WASM through Zenoh. A guest receives pub/sub events
  and publishes another event while handling a request. Each World retains
  independent routing namespace and WASM state.
- WASM itself queries one public `controls` extension point and receives two
  independent native contributions in World A. The same query in World B
  returns an empty result. A malicious guest cannot specify a full public key.
- A WASM query handler now emits multiple incremental Zenoh replies using one
  scoped `reply` import. The Host binds the active query and reply key; the
  guest cannot choose either, and calls outside an active query are denied.
  The integration test receives three separate reply frames in each World.
  A fuel-exhausted guest after an initial reply yields both a partial frame
  and an error frame; consumers must distinguish the two.
- Nested extension points require no transport-level UI concept: a queryable
  returns an opaque contribution containing another point name, and a WASM
  consumer uses the same ordinary public query operation to gather two more
  contributions. The foreign World sees neither the outer point nor its
  contents.
- No matching queryables returns an empty list; errors from every matching
  provider produce a failure, not a false empty result. If some providers
  succeed and others fail, successful contributions are retained; full
  partial-error reporting is not yet implemented in the probe.
- Two queryables answer one QueryTarget::All query without consolidation;
  replies remain available after undeclaring the other handler. A failed
  provider can return a reply error without suppressing a healthy response.
- One selected public role is admitted only for the exact fixture principal
  and registered once. This does not yet integrate real Host provider policy.
- Limits: 4 KiB per message, 16 aggregate replies including errors, 32 KiB
  aggregate reply bytes including errors, eight guest registrations of each
  kind, 350 ms Zenoh query timeout, 32 MiB WASM linear memory and 8 million
  fuel per WASM callback, and eight outbound query/publication operations per
  callback. A deliberately infinite guest loop traps and a ninth transport
  operation is denied, while another world continues responding.
- Explicit guest revocation and queryable/subscriber undeclare are verified.
  No live transport calls or registrations are persisted across a stop.
- **Direct Zenoh comparison:** Two separately launched native processes establish
  their own authenticated Zenoh sessions over loopback TCP. The router uses
  default-deny ACL keyed by each client's username. Each can query its own
  test endpoint, but not the other World's. Wildcard queries do not reveal
  the other World; foreign queryable
  declarations and publications do not reach the foreign consumer. A mismatched
  username/password pair is rejected. Each run generates random passwords,
  supplies the credentials to native over a private pipe, uses a mode-0600
  temporary router credential file, and deletes it after the test. Only TCP
  and user/password Zenoh features are enabled.

One local two-queryable request took approximately 158 microseconds in an
initial run. This is a single uncalibrated observation, not a benchmark.

## Comparison: direct native session vs Host-mediated endpoint

| Criterion | Host-mediated | Direct authenticated Zenoh |
| --- | --- | --- |
| Transport ownership | Host owns session; native sees narrow IPC | Native owns its Zenoh client session |
| Current scope and grant checks | On each admitted action | Router ACL per authenticated user/key/operation |
| Dynamic provider selection | Host can update admission while active | Zenoh ACL is static; changing it requires restart |
| Native networking | May be disabled by the sandbox | Must reach router; this test shares Host networking |
| Operational surface | IPC parser and forwarding | Router config, credentials, ACL and connection lifecycle |

The direct path **does not yet meet Rintawa's untrusted-native isolation
requirement**. This test uses *unencrypted TCP over loopback*. Zenoh's
username/password authentication does not by itself provide TLS confidentiality
or protection against other local processes able to observe/interfere with the
network. Its Bubblewrap sandbox isolates filesystem visibility but deliberately
shares the Host network namespace; arbitrary outbound networking is not
restricted. Neither production-grade native sandboxing nor authenticated
package identity is implemented.

A production direct-session variant would need a limited router endpoint,
credential lifecycle, network isolation, transport confidentiality (such as
mTLS), per-owner resource limits and safe changes to the selected provider.
The Host-mediated variant avoids direct native Zenoh credentials but would
need bounded cancellation, concurrent handling and a minimal IPC protocol.
**Neither architecture is selected for Core by this experiment.**

## Still unproven: requirements before selecting the transport

1. **Untrusted native process isolation and authentication.** This probe now
   starts distinct network-isolated native processes under Host-owned stdio
   channels, but does **not** establish full arbitrary native-code sandbox
   security. It does not authenticate a package installation or dynamically
   authorize raw native Zenoh sessions. If a raw session can reach Host,
   WIT/stdio checks can be bypassed. String prefixes are not ACL boundaries.
2. **Real Host authority and provider choice.** Principal::trusted_fixture
   is not a production authentication mechanism, and no package grants or
   persisted provider selections are connected.
3. **Cancellation of blocking host I/O.** Wasmtime fuel can trap CPU loops;
   it does not interrupt an external blocking host operation. A Zenoh query
   timeout bounds the caller, not the lifetime of arbitrary producer work.
4. **Concurrent backpressure and process lifecycle.** The stdio experiment
   is a deliberately bounded sequential four-operation interaction, not a
   generic native communication framework. There is no proof for many
   simultaneous requests, noisy native peers, process restart, full CPU/memory
   sandbox quotas or direct peer-to-peer native Zenoh ACL. A guest blocked in
   external Host I/O is not interrupted by Wasmtime fuel.
5. **Application-level integration.** No Chat, AI, World command, UI format or
   additional UI extension is implemented. This is a transport proof only.
6. **Community contract identities.** The proposed MVP key has at most two
   pieces: a readable namespaced name and exact ABI fingerprint (conceptually
   `author/operation@hash`). This is not a Core schema registry or a working
   validation protocol. Independent optional capabilities can be independent
   stable contracts rather than historic aliases of an ever-growing API.

If safe native isolation requires a full second routing framework, compare a
simpler Host-mediated transport instead of promoting this experiment into Core.
