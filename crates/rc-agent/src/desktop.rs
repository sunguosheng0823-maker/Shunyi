use anyhow::{ensure, Context, Result};
use rc_access::wire::Access;
use rc_desktop::{Engine, Header, Permission};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

/// Constructed only by a local, explicit setting. A path is never accepted from a remote peer.
#[derive(Clone)]
pub struct DesktopConfig {
    pub engine: PathBuf,
    pub permission: Permission,
    limit: Arc<Semaphore>,
}
impl DesktopConfig {
    pub fn new(engine: PathBuf, permission: Permission) -> Result<Self> {
        ensure!(engine.is_absolute() && engine.is_file(), "远控引擎尚未安装");
        ensure!(permission != Permission::Denied, "远程桌面未启用");
        Ok(Self {
            engine,
            permission,
            limit: Arc::new(Semaphore::new(1)),
        })
    }
}

pub(crate) struct Desktop {
    engine: Engine,
    task: tokio::task::JoinHandle<()>,
    control: bool,
    _permit: OwnedSemaphorePermit,
}
impl Drop for Desktop {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Desktop {
    pub async fn open(
        config: &DesktopConfig,
        id: &str,
        display: u32,
        control: bool,
        output: mpsc::Sender<(Access, Vec<u8>)>,
    ) -> Result<(Self, rc_desktop::DisplayInfo)> {
        ensure!(
            !control || config.permission == Permission::Control,
            "被控设备仅允许查看，未授权键盘和鼠标控制"
        );
        let permit = config
            .limit
            .clone()
            .try_acquire_owned()
            .context("该设备已有桌面会话，请先断开后重试")?;
        let (engine, mut events) = Engine::spawn(&config.engine, "capture")?;
        engine
            .commands
            .send((Header::Start { display, control }, vec![]))
            .await?;
        let first = tokio::time::timeout(Duration::from_secs(12), events.recv())
            .await
            .context("屏幕采集启动超时")?
            .context("远控引擎已停止")?;
        let info = match first.0 {
            Header::Ready { display } => {
                display.validate()?;
                display
            }
            Header::Error { message } => anyhow::bail!(message),
            _ => anyhow::bail!("远控引擎返回了无效响应"),
        };
        let desktop = id.to_owned();
        let task = tokio::spawn(async move {
            let mut reason = "远控引擎已停止".to_string();
            while let Some((header, bytes)) = events.recv().await {
                match header {
                    Header::Frame {
                        sequence,
                        width,
                        height,
                    } => {
                        for (index, part) in bytes.chunks(rc_desktop::FRAGMENT).enumerate() {
                            if output
                                .send((
                                    Access::DesktopFrame {
                                        desktop: desktop.clone(),
                                        sequence,
                                        offset: index * rc_desktop::FRAGMENT,
                                        total: bytes.len(),
                                        width,
                                        height,
                                    },
                                    part.to_vec(),
                                ))
                                .await
                                .is_err()
                            {
                                return;
                            }
                        }
                    }
                    Header::Error { message } => {
                        reason = message;
                        break;
                    }
                    _ => {
                        reason = "远控引擎响应无效".into();
                        break;
                    }
                }
            }
            let _ = output
                .send((Access::DesktopClosed { desktop, reason }, vec![]))
                .await;
        });
        Ok((
            Self {
                engine,
                task,
                control,
                _permit: permit,
            },
            info,
        ))
    }
    pub fn input(&self, event: rc_desktop::Input) -> Result<()> {
        ensure!(self.control, "当前桌面为仅查看模式");
        event.validate()?;
        self.engine
            .commands
            .try_send((Header::Input { event }, vec![]))
            .context("输入队列已满，桌面已断开以释放按键")?;
        Ok(())
    }
}
