use anyhow::{bail, Context, Result};
use rc_access::credentials::CredentialStore;
use std::path::PathBuf;

fn usage() {
    println!("瞬移开源 Agent（免账号）\n\nrc-agent [--state-dir PATH] <command>\n\n  init                         初始化设备身份并显示设备 ID\n  run                          运行设备端（UNIRC_SERVER 指定中继）\n  status                       查看设备 ID、证书及临时密码状态\n  export-certificate FILE      导出连接证书，文件包含私钥，请妥善保管\n  rotate-certificate           轮换长期连接证书，随后重新导出\n  temporary-password [MINUTES]  生成一次性密码，默认 30 分钟内使用\n  revoke-temporary             作废未使用的临时密码\n  rotation HOURS|off           设置长期证书自动轮换，默认关闭\n\n环境变量：UNIRC_STATE_DIR、UNIRC_SERVER、UNIRC_TOKEN（可选中继准入）、UNIRC_DEVICE_NAME、UNIRC_SHELL\n终端使用 Agent 当前系统用户权限；只有管理员以 root 运行 Agent 时才提供 root 终端。");
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let mut args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        usage();
        return Ok(());
    }
    let mut directory = std::env::var_os("UNIRC_STATE_DIR").map(PathBuf::from);
    if args.first().is_some_and(|a| a == "--state-dir") {
        if args.len() < 3 {
            bail!("--state-dir 需要目录和命令");
        }
        directory = Some(PathBuf::from(args.remove(1)));
        args.remove(0);
    }
    let directory = directory.unwrap_or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".shunyi")
    });
    let command = args.first().map(String::as_str).unwrap_or("run");
    let store = CredentialStore::open(directory)?;
    match command {
        "init" | "status" if args.len() <= 1 => {
            println!("{}", serde_json::to_string_pretty(&store.status()?)?)
        }
        "export-certificate" if args.len() == 2 => {
            store.export_certificate(std::path::Path::new(&args[1]))?;
            println!("连接证书已导出。该文件包含私钥，仅分享给受信任的使用者。");
        }
        "rotate-certificate" if args.len() == 1 => {
            store.rotate_certificate()?;
            println!("长期证书已轮换。旧证书不能建立新连接，已有终端继续运行；请重新 export-certificate。");
        }
        "temporary-password" if args.len() <= 2 => {
            let minutes: u64 = args.get(1).map(|v| v.parse()).transpose()?.unwrap_or(30);
            let ttl = minutes.checked_mul(60).context("有效期过大")?;
            let password = store.create_temporary(ttl)?;
            eprintln!("临时密码（{minutes} 分钟内使用，首次认证成功后只授权该次连接）：");
            println!("{password}");
        }
        "revoke-temporary" if args.len() == 1 => {
            store.revoke_temporary()?;
            println!("临时密码已作废。已经授权的访问会话不受影响。");
        }
        "rotation" if args.len() == 2 => {
            store.set_rotation(if args[1] == "off" {
                None
            } else {
                Some(args[1].parse()?)
            })?;
            println!("证书轮换设置已更新，Agent 运行时自动检查。");
        }
        "run" if args.len() <= 1 => {
            let server_url = std::env::var("UNIRC_SERVER")
                .context("缺少 UNIRC_SERVER，例如 ws://127.0.0.1:8080")?;
            if std::env::var("UNIRC_E2E").is_ok_and(|v| v == "false" || v == "0") {
                bail!("此版本不允许关闭设备身份认证和端到端 TLS");
            }
            let agent = rc_agent::run(rc_agent::AgentConfig {
                server_url,
                token: std::env::var("UNIRC_TOKEN").unwrap_or_default(),
                device_name: std::env::var("UNIRC_DEVICE_NAME")
                    .unwrap_or_else(|_| rc_agent::default_device_name()),
                credentials: store,
            });
            tokio::select! { result = agent => result?, _ = tokio::signal::ctrl_c() => {} }
        }
        _ => {
            usage();
            bail!("命令或参数无效");
        }
    }
    Ok(())
}
