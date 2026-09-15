//! Real relay + real agent + the same access client used by the macOS application.
use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use rc_access::{
    client::{AccessClient, ConnectOptions, Credential, TerminalEvent},
    credentials::{AccessCertificate, CredentialStore},
    wire::{self, Relay},
};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

struct Fixture {
    _dir: tempfile::TempDir,
    store: CredentialStore,
    server: String,
    device: String,
    certificate: AccessCertificate,
    agent: tokio::task::JoinHandle<Result<()>>,
    relay: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.agent.abort();
        self.relay.abort();
    }
}
impl Fixture {
    async fn new() -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let store = CredentialStore::open(dir.path().join("device"))?;
        let device = store.public_identity()?.0;
        let path = dir.path().join("test.shunyi-cert");
        store.export_certificate(&path)?;
        let certificate = AccessCertificate::read(&path)?;
        let (address, relay) = rc_server::bind("127.0.0.1:0".parse()?, String::new()).await?;
        let server = format!("ws://{address}");
        let agent = tokio::spawn(rc_agent::run(rc_agent::AgentConfig {
            server_url: server.clone(),
            token: String::new(),
            device_name: "integration-device".into(),
            credentials: store.clone(),
        }));
        let fixture = Self {
            _dir: dir,
            store,
            server,
            device,
            certificate,
            agent,
            relay,
        };
        // Registration is asynchronous; probe only this explicit device.
        for _ in 0..50 {
            if let Ok(client) = fixture.certificate_client().await {
                drop(client);
                return Ok(fixture);
            }
            tokio::time::sleep(Duration::from_millis(40)).await;
        }
        anyhow::bail!("agent did not register")
    }
    async fn connect(&self, credential: Credential) -> Result<Arc<AccessClient>> {
        AccessClient::connect(ConnectOptions {
            server: self.server.clone(),
            token: String::new(),
            device_id: self.device.clone(),
            credential,
        })
        .await
    }
    async fn certificate_client(&self) -> Result<Arc<AccessClient>> {
        self.connect(Credential::Certificate(self.certificate.clone()))
            .await
    }
    async fn certificate_after_rotation(&self) -> Result<Arc<AccessClient>> {
        let path = self
            ._dir
            .path()
            .join(format!("{}.shunyi-cert", uuid::Uuid::new_v4()));
        self.store.export_certificate(&path)?;
        self.connect(Credential::Certificate(AccessCertificate::read(&path)?))
            .await
    }
}
async fn output_until(events: &mut mpsc::Receiver<TerminalEvent>, needle: &str) -> Result<String> {
    tokio::time::timeout(Duration::from_secs(8), async {
        let mut output = String::new();
        loop {
            match events
                .recv()
                .await
                .context("terminal event stream closed")?
            {
                TerminalEvent::Data(bytes) => {
                    output.push_str(&String::from_utf8_lossy(&bytes));
                    if output.contains(needle) {
                        return Ok(output);
                    }
                }
                TerminalEvent::Closed(reason) => {
                    anyhow::bail!("terminal closed before marker: {reason}")
                }
            }
        }
    })
    .await
    .context("PTY output timed out")?
}
async fn assert_shell(client: &AccessClient, marker: &str) -> Result<()> {
    let (id, mut events) = client.open_terminal(110, 35).await?;
    client
        .write(&id, format!("printf 'proof:%s\\n' '{marker}'\n").as_bytes())
        .await?;
    output_until(&mut events, &format!("proof:{marker}")).await?;
    client.close_terminal(&id).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn certificate_shell_resize_and_multiple_terminals() -> Result<()> {
    let fixture = Fixture::new().await?;
    let client = fixture.certificate_client().await?;
    assert!(!client.remote_user.is_empty());
    let (first, mut first_events) = client.open_terminal(80, 24).await?;
    let (second, mut second_events) = client.open_terminal(80, 24).await?;
    client.resize(&first, 123, 37).await?;
    client
        .write(&first, b"stty size; printf 'size:%s\n' done\n")
        .await?;
    let output = output_until(&mut first_events, "size:done").await?;
    assert!(output.contains("37 123"), "PTY resize was not applied");
    client.close_terminal(&first).await?;
    client
        .write(&second, b"printf 'second:%s\n' alive\n")
        .await?;
    output_until(&mut second_events, "second:alive").await?;
    client.close_terminal(&second).await?;
    tokio::time::timeout(Duration::from_secs(3), async {
        while !client.is_closed() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn temporary_password_claim_is_single_use_and_survives_disconnect() -> Result<()> {
    let fixture = Fixture::new().await?;
    let password = fixture.store.create_temporary(120)?;
    assert!(fixture
        .connect(Credential::Temporary("wrong-password".into()))
        .await
        .is_err());
    assert_eq!(fixture.store.status()?.temporary_state, "unused");
    let (one, two) = tokio::join!(
        fixture.connect(Credential::Temporary(password.clone())),
        fixture.connect(Credential::Temporary(password.clone()))
    );
    assert_ne!(
        one.is_ok(),
        two.is_ok(),
        "concurrent claims must have exactly one winner"
    );
    let client = one.or(two)?;
    let (first, _) = client.open_terminal(80, 24).await?;
    let (second, mut events) = client.open_terminal(80, 24).await?;
    client.close_terminal(&first).await?;
    client
        .write(&second, b"printf 'temporary:%s\n' alive\n")
        .await?;
    output_until(&mut events, "temporary:alive").await?;
    drop(client); // Abrupt network loss, not a graceful logout.
    assert!(fixture
        .connect(Credential::Temporary(password))
        .await
        .is_err());
    let reopened = CredentialStore::open(fixture.store.root())?;
    assert_eq!(reopened.status()?.temporary_state, "used");
    let fresh = fixture.store.create_temporary(120)?;
    assert_shell(
        fixture
            .connect(Credential::Temporary(fresh))
            .await?
            .as_ref(),
        "fresh",
    )
    .await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn certificate_rotation_rejects_old_new_connections_preserves_live_terminal() -> Result<()> {
    let fixture = Fixture::new().await?;
    let client = fixture.certificate_client().await?;
    let (id, mut events) = client.open_terminal(80, 24).await?;
    fixture.store.rotate_certificate()?;
    assert!(fixture.certificate_client().await.is_err());
    client.write(&id, b"printf 'rotation:%s\n' live\n").await?;
    output_until(&mut events, "rotation:live").await?;
    assert_shell(
        fixture.certificate_after_rotation().await?.as_ref(),
        "new-certificate",
    )
    .await?;
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn incorrect_device_certificate_and_legacy_plaintext_are_rejected() -> Result<()> {
    let fixture = Fixture::new().await?;
    let other = Fixture::new().await?;
    assert!(fixture
        .connect(Credential::Certificate(other.certificate.clone()))
        .await
        .is_err());
    let (mut ws, _) =
        tokio_tungstenite::connect_async(wire::endpoint(&fixture.server, "client")?).await?;
    let old = rc_core::Frame::new(
        rc_core::message::ty::HELLO,
        &serde_json::json!({"token":""}),
        vec![],
    )?;
    ws.send(Message::Binary(old.encode().into())).await?;
    let response = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await?
        .context("missing explicit rejection")??;
    assert!(
        matches!(response, Message::Binary(bytes) if matches!(Relay::decode(&bytes)?.0, Relay::Error { .. }))
    );
    Ok(())
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn relay_cannot_use_another_clients_tunnel() -> Result<()> {
    let fixture = Fixture::new().await?;
    async fn raw_client(
        url: &str,
    ) -> Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let (mut socket, _) =
            tokio_tungstenite::connect_async(wire::endpoint(url, "client")?).await?;
        socket
            .send(Message::Binary(
                Relay::Hello {
                    version: wire::VERSION,
                    token: String::new(),
                }
                .packet(vec![])?
                .into(),
            ))
            .await?;
        socket.next().await.context("no welcome")??;
        Ok(socket)
    }
    let mut owner = raw_client(&fixture.server).await?;
    owner
        .send(Message::Binary(
            Relay::Open {
                device_id: fixture.device.clone(),
            }
            .packet(vec![])?
            .into(),
        ))
        .await?;
    let tunnel = match owner.next().await.context("no tunnel")?? {
        Message::Binary(bytes) => match Relay::decode(&bytes)?.0 {
            Relay::Ready { tunnel, .. } => tunnel,
            _ => anyhow::bail!("not ready"),
        },
        _ => anyhow::bail!("not binary"),
    };
    let mut attacker = raw_client(&fixture.server).await?;
    attacker
        .send(Message::Binary(
            Relay::Close { tunnel }.packet(vec![])?.into(),
        ))
        .await?;
    let result = tokio::time::timeout(Duration::from_secs(2), attacker.next())
        .await?
        .context("no rejection")??;
    assert!(
        matches!(result, Message::Binary(bytes) if matches!(Relay::decode(&bytes)?.0, Relay::Error { .. }))
    );
    // The owner remains connected after the rejected cross-client close.
    owner
        .send(Message::Binary(Relay::Ping.packet(vec![])?.into()))
        .await?;
    let result = owner.next().await.context("owner disconnected")??;
    assert!(
        matches!(result, Message::Binary(bytes) if matches!(Relay::decode(&bytes)?.0, Relay::Ping))
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wss_upgrade_uses_the_real_router_and_verified_tls() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let store = CredentialStore::open(dir.path().join("tls"))?;
    let (id, ca) = store.public_identity()?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let address = listener.local_addr()?;
    let acceptor = tokio_rustls::TlsAcceptor::from(store.server_config()?);
    let server = tokio::spawn(async move {
        let (socket, _) = listener.accept().await?;
        let tls = acceptor.accept(socket).await?;
        rc_server::handle_tls_connection(rc_server::AppState::new(String::new()), tls).await
    });
    let tcp = tokio::net::TcpStream::connect(address).await?;
    let tls = tokio_rustls::TlsConnector::from(rc_access::credentials::temporary_client_config(
        &id, &ca,
    )?)
    .connect(rc_access::credentials::SERVER_NAME.try_into()?, tcp)
    .await?;
    let (mut socket, _) = tokio_tungstenite::client_async(
        format!("wss://shunyi-device.local:{}/client/ws", address.port()),
        tls,
    )
    .await?;
    socket
        .send(Message::Binary(
            Relay::Hello {
                version: wire::VERSION,
                token: String::new(),
            }
            .packet(vec![])?
            .into(),
        ))
        .await?;
    let response = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await?
        .context("missing WSS welcome")??;
    assert!(
        matches!(response, Message::Binary(bytes) if matches!(Relay::decode(&bytes)?.0, Relay::Welcome))
    );
    socket.close(None).await?;
    server.abort();
    Ok(())
}
