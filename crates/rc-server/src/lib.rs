//! Certificate-authenticated device registry and bounded, opaque TLS byte forwarding.
use anyhow::{bail, ensure, Context, Result};
use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
    routing::get,
    Router,
};
use futures_util::{SinkExt, StreamExt};
use rc_access::{
    credentials,
    wire::{self, Relay},
};
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::{
    sync::{mpsc, Mutex},
    time::timeout,
};

#[derive(Clone)]
struct Agent {
    version: u8,
    instance: String,
    ca: String,
    tx: mpsc::Sender<Vec<u8>>,
}
struct Route {
    device: String,
    instance: String,
    client: String,
    agent_tx: mpsc::Sender<Vec<u8>>,
    client_tx: mpsc::Sender<Vec<u8>>,
}
#[derive(Default)]
struct Registry {
    agents: HashMap<String, Agent>,
    routes: HashMap<String, Route>,
}
#[derive(Clone)]
pub struct AppState {
    registry: Arc<Mutex<Registry>>,
    token: Arc<String>,
}
impl AppState {
    pub fn new(token: String) -> Self {
        Self {
            registry: Arc::default(),
            token: Arc::new(token),
        }
    }
}
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/agent/ws", get(agent_ws))
        .route("/client/ws", get(client_ws))
        .route("/health", get(|| async { "ok" }))
        .with_state(state)
}
pub async fn bind(
    bind: SocketAddr,
    token: String,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>)> {
    let listener = tokio::net::TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let task = tokio::spawn(async move {
        if let Err(error) = axum::serve(listener, router(AppState::new(token))).await {
            tracing::error!(%error, "relay stopped");
        }
    });
    Ok((addr, task))
}
pub async fn serve(addr: SocketAddr, token: String) -> Result<()> {
    bind(addr, token).await?.1.await?;
    Ok(())
}
pub async fn handle_tls_connection(
    state: AppState,
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
) -> Result<()> {
    hyper_util::server::conn::auto::Builder::new(hyper_util::rt::TokioExecutor::new())
        .serve_connection_with_upgrades(
            hyper_util::rt::TokioIo::new(stream),
            hyper_util::service::TowerToHyperService::new(router(state)),
        )
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))
}
async fn agent_ws(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade
        .max_message_size(wire::MAX_PACKET)
        .max_frame_size(wire::MAX_PACKET)
        .on_upgrade(move |ws| socket(state, ws, true))
}
async fn client_ws(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade
        .max_message_size(wire::MAX_PACKET)
        .max_frame_size(wire::MAX_PACKET)
        .on_upgrade(move |ws| socket(state, ws, false))
}
async fn receive(
    stream: &mut futures_util::stream::SplitStream<WebSocket>,
) -> Result<(Relay, Vec<u8>)> {
    loop {
        match stream.next().await.context("peer disconnected")?? {
            Message::Binary(bytes) => return Relay::decode(&bytes),
            Message::Ping(_) | Message::Pong(_) => {}
            _ => bail!("peer disconnected"),
        }
    }
}
fn enqueue(tx: &mpsc::Sender<Vec<u8>>, message: Relay, bytes: Vec<u8>) -> Result<()> {
    tx.try_send(message.packet(bytes)?)
        .context("relay queue full or disconnected")
}
async fn socket(state: AppState, ws: WebSocket, is_agent: bool) {
    let (mut sink, mut stream) = ws.split();
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(wire::QUEUE);
    let writer = async {
        while let Some(bytes) = rx.recv().await {
            sink.send(Message::Binary(bytes.into())).await?;
        }
        Ok::<_, anyhow::Error>(())
    };
    let reader = async {
        let mut peer_version = wire::VERSION;
        let auth = async {
            if is_agent {
                let nonce = credentials::random_secret();
                enqueue(
                    &tx,
                    Relay::Challenge {
                        nonce: nonce.clone(),
                    },
                    vec![],
                )?;
                match receive(&mut stream).await?.0 {
                    Relay::Register {
                        version,
                        device_id,
                        name,
                        ca_pem,
                        signature,
                        token,
                    } => {
                        ensure!(
                            wire::supported(version)
                                && (state.token.is_empty() || token == *state.token),
                            "中继接入验证失败"
                        );
                        credentials::verify_registration(
                            &device_id, &ca_pem, &nonce, &name, &signature,
                        )?;
                        let mut registry = state.registry.lock().await;
                        ensure!(registry.agents.len() < 4096, "中继设备容量已满");
                        ensure!(
                            !registry.agents.contains_key(&device_id),
                            "该设备已有在线连接"
                        );
                        registry.agents.insert(
                            device_id,
                            Agent {
                                version,
                                instance: id.clone(),
                                ca: ca_pem,
                                tx: tx.clone(),
                            },
                        );
                        enqueue(&tx, Relay::Registered, vec![])?;
                    }
                    _ => bail!("请使用支持证书认证的 Agent"),
                }
            } else {
                match receive(&mut stream).await?.0 {
                    Relay::Hello { version, token }
                        if wire::supported(version)
                            && (state.token.is_empty() || token == *state.token) =>
                    {
                        peer_version = version;
                        enqueue(&tx, Relay::Welcome, vec![])?
                    }
                    _ => bail!("中继接入验证失败或客户端版本过旧"),
                }
            }
            Ok::<_, anyhow::Error>(())
        };
        timeout(Duration::from_secs(10), auth)
            .await
            .context("中继握手超时")??;
        loop {
            let (message, data) = timeout(Duration::from_secs(60), receive(&mut stream))
                .await
                .context("中继心跳超时")??;
            match message {
                Relay::Ping => enqueue(&tx, Relay::Ping, vec![])?,
                Relay::Open { device_id } if !is_agent => {
                    let mut registry = state.registry.lock().await;
                    if registry.routes.values().filter(|r| r.client == id).count() >= 1 {
                        bail!("每条客户端连接只允许一个加密设备通道");
                    }
                    let Some(agent) = registry.agents.get(&device_id).cloned() else {
                        enqueue(
                            &tx,
                            Relay::Error {
                                code: "offline".into(),
                                message: "设备当前离线，请先启动设备端 Agent".into(),
                            },
                            vec![],
                        )?;
                        continue;
                    };
                    if peer_version >= wire::DESKTOP_VERSION
                        && agent.version < wire::DESKTOP_VERSION
                    {
                        enqueue(
                            &tx,
                            Relay::Error {
                                code: "desktop_unavailable".into(),
                                message: "设备尚未启用远程桌面，请先在被控设备开启共享".into(),
                            },
                            vec![],
                        )?;
                        continue;
                    }
                    if registry
                        .routes
                        .values()
                        .filter(|r| r.device == device_id)
                        .count()
                        >= 32
                    {
                        enqueue(
                            &tx,
                            Relay::Error {
                                code: "busy".into(),
                                message: "设备连接数量已达到上限".into(),
                            },
                            vec![],
                        )?;
                        continue;
                    }
                    let tunnel = uuid::Uuid::new_v4().to_string();
                    enqueue(
                        &agent.tx,
                        Relay::Incoming {
                            tunnel: tunnel.clone(),
                        },
                        vec![],
                    )?;
                    enqueue(
                        &tx,
                        Relay::Ready {
                            tunnel: tunnel.clone(),
                            device_id: device_id.clone(),
                            ca_pem: agent.ca,
                        },
                        vec![],
                    )?;
                    registry.routes.insert(
                        tunnel,
                        Route {
                            device: device_id,
                            instance: agent.instance,
                            client: id.clone(),
                            agent_tx: agent.tx,
                            client_tx: tx.clone(),
                        },
                    );
                }
                Relay::Data { ref tunnel } | Relay::Close { ref tunnel } => {
                    let mut registry = state.registry.lock().await;
                    if let Some(route) = registry.routes.get(tunnel) {
                        ensure!(
                            if is_agent {
                                route.instance == id
                            } else {
                                route.client == id
                            },
                            "该通道不属于此连接"
                        );
                        let destination = if is_agent {
                            &route.client_tx
                        } else {
                            &route.agent_tx
                        };
                        let destination = destination.clone();
                        let packet = message.packet(data)?;
                        if matches!(message, Relay::Close { .. }) {
                            registry.routes.remove(tunnel);
                        }
                        drop(registry);
                        timeout(Duration::from_secs(10), destination.send(packet))
                            .await
                            .context("中继目标长时间拥塞")?
                            .context("中继目标已断开")?;
                    }
                }
                _ => bail!("拒绝旧版明文终端或未授权的协议消息"),
            }
        }
    };
    tokio::pin!(writer);
    let (result, writer_finished) =
        tokio::select! { r = &mut writer => (r, true), r = reader => (r, false) };
    if let Err(error) = result {
        let _ = enqueue(
            &tx,
            Relay::Error {
                code: "rejected".into(),
                message: error.to_string(),
            },
            vec![],
        );
        // Let the queued error flush, then close the socket. No detached writer retains routes.
        if !writer_finished {
            let _ = timeout(Duration::from_millis(50), &mut writer).await;
        }
    }
    let mut registry = state.registry.lock().await;
    registry.agents.retain(|_, a| a.instance != id);
    registry.routes.retain(|tunnel, route| {
        let remove = if is_agent {
            route.instance == id
        } else {
            route.client == id
        };
        if remove {
            let destination = if is_agent {
                &route.client_tx
            } else {
                &route.agent_tx
            };
            let _ = enqueue(
                destination,
                Relay::Close {
                    tunnel: tunnel.clone(),
                },
                vec![],
            );
        }
        !remove
    });
}
