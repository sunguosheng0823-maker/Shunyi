//! rc-server 入口：读取环境变量并启动。
//!
//! 环境变量：
//! UNIRC_BIND       监听地址（默认 127.0.0.1:8080）
//! UNIRC_TOKEN      可选的中继准入 token，不授予设备终端权限
//! UNIRC_TLS_CERT   TLS 证书文件路径（可选，启用 WSS）
//! UNIRC_TLS_KEY    TLS 私钥文件路径（可选，启用 WSS）

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;
use tracing::info;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let bind: SocketAddr = std::env::var("UNIRC_BIND")
        .unwrap_or_else(|_| "127.0.0.1:8080".into())
        .parse()?;
    let token = std::env::var("UNIRC_TOKEN").unwrap_or_default();

    // TLS 配置
    let tls_cert = std::env::var("UNIRC_TLS_CERT").ok();
    let tls_key = std::env::var("UNIRC_TLS_KEY").ok();

    anyhow::ensure!(
        tls_cert.is_some() == tls_key.is_some(),
        "UNIRC_TLS_CERT 和 UNIRC_TLS_KEY 必须同时设置"
    );
    if let (Some(cert_path), Some(key_path)) = (tls_cert, tls_key) {
        info!("TLS 模式启动");
        serve_tls(bind, token, &cert_path, &key_path).await
    } else {
        info!("HTTP 中继已启动；跨公网部署请配置 WSS 或 HTTPS 反向代理");
        rc_server::serve(bind, token).await
    }
}

async fn serve_tls(
    bind: SocketAddr,
    token: String,
    cert_path: &str,
    key_path: &str,
) -> anyhow::Result<()> {
    // 读取证书和私钥
    let cert_file = std::fs::File::open(cert_path)?;
    let key_file = std::fs::File::open(key_path)?;

    let mut cert_reader = std::io::BufReader::new(cert_file);
    let mut key_reader = std::io::BufReader::new(key_file);

    let certs = rustls_pemfile::certs(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;
    let key = rustls_pemfile::private_key(&mut key_reader)?
        .ok_or_else(|| anyhow::anyhow!("未找到私钥"))?;

    // 配置 TLS
    let config = tokio_rustls::rustls::ServerConfig::builder_with_provider(Arc::new(
        tokio_rustls::rustls::crypto::ring::default_provider(),
    ))
    .with_protocol_versions(&[&tokio_rustls::rustls::version::TLS13])?
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .map_err(|e| anyhow::anyhow!("TLS 配置错误: {e}"))?;

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;

    info!("rc-server TLS 监听 https://{addr}");

    // 创建共享状态
    let state = rc_server::AppState::new(token);

    loop {
        let (stream, _peer_addr) = listener.accept().await?;
        let acceptor = acceptor.clone();
        let state = state.clone();

        tokio::spawn(async move {
            match tokio::time::timeout(std::time::Duration::from_secs(10), acceptor.accept(stream))
                .await
            {
                Ok(Ok(tls_stream)) => {
                    // 处理 TLS 连接
                    if let Err(e) = rc_server::handle_tls_connection(state, tls_stream).await {
                        tracing::error!("TLS 连接处理错误: {e}");
                    }
                }
                _ => tracing::debug!("外层 TLS 握手失败或超时"),
            }
        });
    }
}
