//! Local, explicit desktop acceptance against a disposable native_fixture.
//! Pass --input only with the Shunyi control fixture in front and user approval.
use anyhow::{Context, Result};
use rc_access::{
    client::{AccessClient, ConnectOptions, Credential, DesktopEvent},
    credentials::AccessCertificate,
};
use rc_desktop::{Engine, Header, Input};
use std::{path::PathBuf, time::Duration};

#[tokio::main]
async fn main() -> Result<()> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("supply a disposable fixture directory")?,
    );
    let control = std::env::args().any(|a| a == "--input");
    let fixture: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("fixture.json"))?)?;
    let engine = PathBuf::from(
        std::env::var_os("SHUNYI_DESKTOP_ENGINE").context("set SHUNYI_DESKTOP_ENGINE")?,
    );
    let client = AccessClient::connect_desktop(ConnectOptions {
        server: fixture["server"].as_str().context("server")?.into(),
        token: String::new(),
        device_id: fixture["deviceId"].as_str().context("deviceId")?.into(),
        credential: Credential::Certificate(AccessCertificate::read(std::path::Path::new(
            fixture["certificatePath"].as_str().context("certificate")?,
        ))?),
    })
    .await
    .context("桌面验收认证连接失败")?;
    let (desktop, mut events) = client.open_desktop(0, control).await?;
    let mut frames = 0;
    let (decoder, mut decoded) = Engine::spawn(&engine, "decode")?;
    let started = std::time::Instant::now();
    let mut resolution = (0, 0);
    let mut display_coordinates = (0, 0, 1.0_f32);
    let mut first_ms = 0;
    let mut distinct_colors = 0;
    while frames < 3 {
        match tokio::time::timeout(Duration::from_secs(20), events.recv())
            .await?
            .context("desktop closed")?
        {
            DesktopEvent::Opened {
                display,
                control: granted,
            } => {
                anyhow::ensure!(granted == control, "unexpected control permission");
                resolution = (display.width, display.height);
                display_coordinates = (display.origin_x, display.origin_y, display.scale);
            }
            DesktopEvent::Frame {
                sequence,
                width,
                height,
                data,
            } => {
                decoder
                    .commands
                    .send((
                        Header::Decode {
                            sequence,
                            width,
                            height,
                        },
                        data,
                    ))
                    .await?;
                let (header, rgba) = tokio::time::timeout(Duration::from_secs(10), decoded.recv())
                    .await?
                    .context("decoder closed")?;
                anyhow::ensure!(
                    matches!(header, Header::Image { .. })
                        && rgba.len() == width as usize * height as usize * 4,
                    "invalid decoded frame"
                );
                distinct_colors = rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .step_by(131)
                    .map(|pixel| [pixel[0], pixel[1], pixel[2]])
                    .collect::<std::collections::HashSet<_>>()
                    .len();
                anyhow::ensure!(
                    distinct_colors > 4,
                    "captured frame is blank or lacks the test window"
                );
                frames += 1;
                if frames == 1 {
                    first_ms = started.elapsed().as_millis();
                    std::fs::write(
                        root.join("desktop-first-frame.json"),
                        serde_json::to_vec_pretty(
                            &serde_json::json!({"real_unirc_tls":true,"real_rustdesk_capture":true,"real_vp8_decode":true,"width":width,"height":height,"first_frame_ms":first_ms,"distinct_sampled_colors":distinct_colors}),
                        )?,
                    )?;
                    // Save no user screen content: retain frame measurements only.
                    if control {
                        let target: serde_json::Value = serde_json::from_slice(&std::fs::read(
                            root.join("control-target.json"),
                        )?)?;
                        anyhow::ensure!(
                            target["pid"] == target["front_pid"],
                            "test fixture did not acquire foreground; refusing input"
                        );
                        let x = (target["x"].as_f64().context("fixture x")? as f32
                            - display_coordinates.0 as f32)
                            * display_coordinates.2
                            / (width - 1) as f32;
                        let y = (target["y"].as_f64().context("fixture y")? as f32
                            - display_coordinates.1 as f32)
                            * display_coordinates.2
                            / (height - 1) as f32;
                        client
                            .desktop_input(&desktop, Input::Pointer { x, y })
                            .await?;
                        client
                            .desktop_input(
                                &desktop,
                                Input::Button {
                                    button: rc_desktop::Button::Left,
                                    down: true,
                                },
                            )
                            .await?;
                        client
                            .desktop_input(
                                &desktop,
                                Input::Button {
                                    button: rc_desktop::Button::Left,
                                    down: false,
                                },
                            )
                            .await?;
                        client
                            .desktop_input(
                                &desktop,
                                Input::Key {
                                    key: "KeyA".into(),
                                    down: true,
                                },
                            )
                            .await?;
                        client
                            .desktop_input(
                                &desktop,
                                Input::Key {
                                    key: "KeyA".into(),
                                    down: false,
                                },
                            )
                            .await?;
                        client.desktop_input(&desktop, Input::ReleaseAll).await?;
                        // Deliberately leave B pressed; closing must release it in the helper.
                        client
                            .desktop_input(
                                &desktop,
                                Input::Key {
                                    key: "KeyB".into(),
                                    down: true,
                                },
                            )
                            .await?;
                    }
                }
                // Capture only emits changed frames. One complete frame is enough for a static desktop.
                if !control || frames >= 1 {
                    break;
                }
            }
            DesktopEvent::Closed(reason) => anyhow::bail!(reason),
        }
    }
    if control {
        // A real viewer keeps consuming frames while keys are held. Leaving this
        // channel unread fills the bounded video queue and invalidates the input test.
        let held = tokio::time::sleep(Duration::from_millis(800));
        tokio::pin!(held);
        loop {
            tokio::select! {
                _ = &mut held => break,
                event = events.recv() => match event.context("键鼠验收期间桌面事件流已关闭")? {
                    DesktopEvent::Closed(reason) => anyhow::bail!("键鼠验收期间桌面中断：{reason}"),
                    DesktopEvent::Frame { .. } | DesktopEvent::Opened { .. } => {}
                }
            }
        }
    }
    let disconnect_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs_f64();
    client
        .close_desktop(&desktop)
        .await
        .context("桌面验收主动断开失败")?;
    let receipt = serde_json::json!({"real_unirc_tls":true,"real_rustdesk_capture":true,"real_vp8_decode":true,"frames":frames,"width":resolution.0,"height":resolution.1,"first_frame_ms":first_ms,"distinct_sampled_colors":distinct_colors,"control_requested":control,"disconnect_at":disconnect_at,"input_acceptance_requires_fixture_log":control});
    std::fs::write(
        root.join(if control {
            "desktop-control.json"
        } else {
            "desktop-view.json"
        }),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    println!("{}", receipt);
    Ok(())
}
