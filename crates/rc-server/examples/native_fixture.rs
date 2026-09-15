//! Disposable loopback fixture for native-client acceptance. Never use as a deployment.
use anyhow::Result;
use rc_access::credentials::CredentialStore;
use std::{fs, io::Write, path::PathBuf};

#[tokio::main]
async fn main() -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("rc_agent=debug,rc_server=debug")
        .with_ansi(false)
        .try_init();
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("provide a disposable fixture directory"),
    );
    let store = CredentialStore::open(root.join("agent"))?;
    let path = root.join("acceptance.shunyi-cert");
    if !path.exists() {
        store.export_certificate(&path)?;
    }
    let password = store.create_temporary(1800)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(root.join("temporary.test-secret"))?
        .write_all(password.as_bytes())?;
    let (address, relay) = rc_server::bind("127.0.0.1:0".parse()?, String::new()).await?;
    let server = format!("ws://{address}");
    fs::write(
        root.join("fixture.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"server":server,"deviceId":store.public_identity()?.0,"certificatePath":path}),
        )?,
    )?;
    println!("Disposable loopback fixture: {server}");
    let agent = rc_agent::run(rc_agent::AgentConfig {
        desktop: std::env::var_os("SHUNYI_DESKTOP_ENGINE")
            .map(|path| {
                rc_agent::desktop::DesktopConfig::new(
                    PathBuf::from(path),
                    rc_desktop::Permission::Control,
                )
            })
            .transpose()?,
        server_url: server,
        token: String::new(),
        device_name: "Native acceptance fixture".into(),
        credentials: store,
    });
    tokio::select! { result = agent => result?, _ = tokio::signal::ctrl_c() => {} }
    relay.abort();
    Ok(())
}
