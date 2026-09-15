//! Optional AGPL desktop engine using pinned RustDesk capture, VP8 codec, and input modules.
mod ipc_output;
use anyhow::{bail, ensure, Context, Result};
use base::message_proto::{video_frame, VideoFrame};
use enigo::{Enigo, Key, KeyboardControllable, MouseButton, MouseControllable};
use protobuf::Message;
use rc_desktop::{Button, DisplayInfo, Header, Input, Packet};
use scrap::{
    codec::{Decoder, Encoder, EncoderCfg},
    CodecFormat, ImageFormat, ImageRgb, ImageTexture, TraitCapturer, VpxEncoderConfig,
    VpxVideoCodecId,
};
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    if let Err(error) = run().await {
        if let Ok(mut stdout) = ipc_output::writer() {
            let _ = tokio::time::timeout(
                Duration::from_millis(250),
                rc_desktop::write_packet(
                    &mut stdout,
                    &(
                        Header::Error {
                            message: format!("{error:#}"),
                        },
                        vec![],
                    ),
                ),
            )
            .await;
        }
        std::process::exit(1);
    }
    // run() has dropped capture and input guards. Tokio's blocking stdio worker may
    // still wait on an abandoned output pipe; it must not delay helper termination.
    std::process::exit(0);
}
async fn run() -> Result<()> {
    match std::env::args().nth(1).as_deref() {
        Some("capture") => capture().await,
        Some("decode") => decode().await,
        Some("self-test") => self_test().await,
        Some("permissions") => {
            request_permissions();
            println!("{}", screen_allowed());
            Ok(())
        }
        _ => bail!("请选择 capture、decode 或 self-test"),
    }
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn AXIsProcessTrusted() -> bool;
}
fn screen_allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { CGPreflightScreenCaptureAccess() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}
fn input_allowed() -> bool {
    #[cfg(target_os = "macos")]
    {
        unsafe { AXIsProcessTrusted() }
    }
    #[cfg(target_os = "linux")]
    unsafe {
        let library = libc::dlopen(
            b"libxdo.so.3\0".as_ptr().cast(),
            libc::RTLD_LAZY | libc::RTLD_LOCAL,
        );
        if library.is_null() {
            return false;
        }
        libc::dlclose(library);
        true
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        true
    }
}
fn request_permissions() {
    #[cfg(target_os = "macos")]
    unsafe {
        CGRequestScreenCaptureAccess();
    }
}

fn check_desktop_session() -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        ensure!(
            std::env::var("DISPLAY").is_ok_and(|v| !v.trim().is_empty()),
            "当前没有可共享的 X11 桌面，请在已登录的图形会话中运行瞬移"
        );
        ensure!(
            std::env::var("XDG_SESSION_TYPE").as_deref() != Ok("wayland")
                && std::env::var_os("WAYLAND_DISPLAY").is_none(),
            "Linux 预览版暂支持 X11，请在登录界面选择 Xorg 会话；Wayland 桌面共享尚未接入"
        );
    }
    #[cfg(windows)]
    unsafe {
        #[link(name = "user32")]
        extern "system" {
            fn SetProcessDpiAwarenessContext(context: isize) -> i32;
        }
        // DXGI reports physical pixels; keep mouse coordinates in the same space.
        SetProcessDpiAwarenessContext(-4);
    }
    Ok(())
}

async fn capture() -> Result<()> {
    let mut stdout = ipc_output::writer()?;
    let mut stdin = tokio::io::stdin();
    let (
        Header::Start {
            display: index,
            control,
        },
        _,
    ) = rc_desktop::read_packet(&mut stdin).await?
    else {
        bail!("缺少桌面启动参数")
    };
    ensure!(
        screen_allowed(),
        "请在 macOS 系统设置 → 隐私与安全性 → 屏幕与系统音频录制中允许瞬移，重新打开共享"
    );
    ensure!(
        !control || input_allowed(),
        "{}",
        if cfg!(target_os = "linux") {
            "缺少 X11 键鼠运行库 libxdo.so.3，请安装 libxdo3 或使用完整 AppImage 安装包"
        } else {
            "请在 macOS 系统设置 → 隐私与安全性 → 辅助功能中允许瞬移控制键盘和鼠标"
        }
    );
    check_desktop_session()?;
    let displays = scrap::Display::all().context("无法枚举屏幕")?;
    let display = displays
        .into_iter()
        .nth(index as usize)
        .context("所选屏幕已断开")?;
    let origin = display.origin();
    #[cfg(target_os = "macos")]
    let scale = display.scale() as f32;
    #[cfg(not(target_os = "macos"))]
    let scale = 1.0;
    let info = DisplayInfo {
        width: display.width() as u32,
        height: display.height() as u32,
        origin_x: origin.0,
        origin_y: origin.1,
        scale,
        cursor_embedded: false,
    };
    info.validate()?;
    let mut capturer = scrap::Capturer::new(display).context("无法启动屏幕采集，请确认录屏权限")?;
    let mut encoder = encoder(info.width, info.height)?;
    let mut input = control.then(|| InputGuard::new(info.clone()));
    rc_desktop::write_packet(
        &mut stdout,
        &(
            Header::Ready {
                display: info.clone(),
            },
            vec![],
        ),
    )
    .await?;
    let (tx, mut commands) = tokio::sync::mpsc::channel::<Packet>(32);
    let (ended, mut input_ended) = tokio::sync::oneshot::channel();
    let reader = tokio::spawn(async move {
        while let Ok(packet) = rc_desktop::read_packet(&mut stdin).await {
            if tx.try_send(packet).is_err() {
                break;
            }
        }
        let _ = ended.send(());
    });
    let result = async {
        let start = Instant::now();
        let mut sequence = 0;
        let mut yuv = vec![];
        let mut mid = vec![];
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = &mut input_ended => break,
                packet = commands.recv() => match packet {
                    None | Some((Header::Stop, _)) => break,
                    Some((Header::Input { event }, _)) => input.as_mut().context("此会话仅允许查看")?.apply(event)?,
                    _ => bail!("不支持的引擎命令"),
                },
                _ = tick.tick() => match capturer.frame(Duration::from_millis(100)) {
                    Ok(frame) => {
                        let frame = frame.to(encoder.yuvfmt(), &mut yuv, &mut mid)?;
                        let encoded = encoder.encode_to_message(frame, start.elapsed().as_millis() as i64)?.write_to_bytes()?;
                        ensure!(encoded.len() <= rc_desktop::MAX_ENCODED, "压缩画面过大");
                        sequence += 1;
                        let packet = (Header::Frame { sequence, width: info.width, height: info.height }, encoded);
                        // Release input on disconnect even when a stalled decoder blocks stdout.
                        tokio::select! {
                            biased;
                            _ = &mut input_ended => break,
                            result = rc_desktop::write_packet(&mut stdout, &packet) => result?,
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        ensure!(sequence > 0 || start.elapsed() < Duration::from_secs(10), "屏幕采集未返回画面，请重新授予录屏权限");
                    }
                    Err(error) => {
                        // Follow RustDesk's Windows capture recovery: DXGI can stop
                        // after a desktop update even when its first frame succeeded.
                        // Fall back once; a GDI failure still terminates the session.
                        #[cfg(target_os = "windows")]
                        if !capturer.is_gdi() && capturer.set_gdi() {
                            eprintln!("DXGI capture failed; using GDI: {error}");
                            continue;
                        }
                        return Err(error).context("屏幕采集已停止");
                    }
                }
            }
        }
        Ok(())
    }.await;
    reader.abort();
    drop(input); // releases our pressed keys and mouse buttons on every exit path
    result
}
fn encoder(width: u32, height: u32) -> Result<Encoder> {
    Encoder::new(
        EncoderCfg::VPX(VpxEncoderConfig {
            width,
            height,
            quality: 0.65,
            codec: VpxVideoCodecId::VP8,
            keyframe_interval: Some(30),
        }),
        false,
    )
}

async fn decode() -> Result<()> {
    let mut stdin = tokio::io::stdin();
    let mut stdout = ipc_output::writer()?;
    let mut decoder = Decoder::new(CodecFormat::VP8, None);
    let mut rgb = ImageRgb::new(ImageFormat::ABGR, 1);
    let mut texture = ImageTexture::default();
    let mut chroma = None;
    loop {
        let packet = rc_desktop::read_packet(&mut stdin).await;
        let (header, data) = match packet {
            Ok(p) => p,
            Err(e)
                if e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::UnexpectedEof) =>
            {
                break
            }
            Err(e) => return Err(e),
        };
        match header {
            Header::Stop => break,
            Header::Decode {
                sequence,
                width,
                height,
            } => {
                let video = VideoFrame::parse_from_bytes(&data).context("无效编码画面")?;
                let Some(frame @ video_frame::Union::Vp8s(_)) = video.union.as_ref() else {
                    bail!("预览版仅支持 VP8 画面")
                };
                if let video_frame::Union::Vp8s(frames) = frame {
                    ensure!(
                        !frames.frames.is_empty() && frames.frames.len() <= 8,
                        "编码帧数量无效"
                    );
                    for frame in &frames.frames {
                        validate_vp8(&frame.data, width, height)?;
                    }
                }
                let mut pixelbuffer = true;
                if decoder.handle_video_frame(
                    frame,
                    &mut rgb,
                    &mut texture,
                    &mut pixelbuffer,
                    &mut chroma,
                )? {
                    ensure!(
                        rgb.w == width as usize
                            && rgb.h == height as usize
                            && rgb.raw.len() == rgb.w * rgb.h * 4,
                        "解码分辨率与消息不符"
                    );
                    rc_desktop::write_packet(
                        &mut stdout,
                        &(
                            Header::Image {
                                sequence,
                                width,
                                height,
                            },
                            std::mem::take(&mut rgb.raw),
                        ),
                    )
                    .await?;
                }
            }
            _ => bail!("不支持的解码命令"),
        }
    }
    Ok(())
}

struct InputGuard {
    enigo: Enigo,
    info: DisplayInfo,
    keys: HashMap<String, Key>,
    buttons: HashSet<Button>,
}
impl InputGuard {
    fn new(info: DisplayInfo) -> Self {
        Self {
            enigo: Enigo::new(),
            info,
            keys: HashMap::new(),
            buttons: HashSet::new(),
        }
    }
    fn apply(&mut self, event: Input) -> Result<()> {
        event.validate()?;
        match event {
            Input::Pointer { x, y } => self.enigo.mouse_move_to(
                self.info.origin_x
                    + (x * (self.info.width - 1) as f32 / self.info.scale).round() as i32,
                self.info.origin_y
                    + (y * (self.info.height - 1) as f32 / self.info.scale).round() as i32,
            ),
            Input::Button { button, down } => {
                if down {
                    if self.buttons.insert(button) {
                        self.enigo
                            .mouse_down(mouse_button(button))
                            .map_err(|e| anyhow::anyhow!("鼠标输入失败：{e:?}"))?;
                    }
                } else if self.buttons.remove(&button) {
                    self.enigo.mouse_up(mouse_button(button));
                }
            }
            Input::Scroll { x, y } => {
                MouseControllable::mouse_scroll_x(&mut self.enigo, x);
                MouseControllable::mouse_scroll_y(&mut self.enigo, y);
            }
            Input::Key { key, down } => {
                if down {
                    let native = keyboard_key(&key)?;
                    self.keys.insert(key, native);
                    self.enigo
                        .key_down(native)
                        .map_err(|e| anyhow::anyhow!("键盘输入失败：{e:?}"))?;
                } else if let Some(native) = self.keys.remove(&key) {
                    self.enigo.key_up(native);
                }
            }
            Input::Text { text } => self.enigo.key_sequence(&text),
            Input::ReleaseAll => self.release(),
        }
        Ok(())
    }
    fn release(&mut self) {
        for (_, key) in self.keys.drain() {
            self.enigo.key_up(key);
        }
        for button in self.buttons.drain() {
            self.enigo.mouse_up(mouse_button(button));
        }
    }
}
impl Drop for InputGuard {
    fn drop(&mut self) {
        self.release();
    }
}
fn mouse_button(button: Button) -> MouseButton {
    match button {
        Button::Left => MouseButton::Left,
        Button::Middle => MouseButton::Middle,
        Button::Right => MouseButton::Right,
    }
}
fn keyboard_key(value: &str) -> Result<Key> {
    Ok(match value {
        "ControlLeft" | "ControlRight" => Key::Control,
        "ShiftLeft" | "ShiftRight" => Key::Shift,
        "AltLeft" | "AltRight" => Key::Alt,
        "MetaLeft" | "MetaRight" => Key::Meta,
        "Enter" | "NumpadEnter" => Key::Return,
        "Escape" => Key::Escape,
        "Tab" => Key::Tab,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Space" => Key::Space,
        "ArrowUp" => Key::UpArrow,
        "ArrowDown" => Key::DownArrow,
        "ArrowLeft" => Key::LeftArrow,
        "ArrowRight" => Key::RightArrow,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        "F1" => Key::F1,
        "F2" => Key::F2,
        "F3" => Key::F3,
        "F4" => Key::F4,
        "F5" => Key::F5,
        "F6" => Key::F6,
        "F7" => Key::F7,
        "F8" => Key::F8,
        "F9" => Key::F9,
        "F10" => Key::F10,
        "F11" => Key::F11,
        "F12" => Key::F12,
        "Minus" => Key::Layout('-'),
        "Equal" => Key::Layout('='),
        "BracketLeft" => Key::Layout('['),
        "BracketRight" => Key::Layout(']'),
        "Backslash" => Key::Layout('\\'),
        "Semicolon" => Key::Layout(';'),
        "Quote" => Key::Layout('\''),
        "Backquote" => Key::Layout('`'),
        "Comma" => Key::Layout(','),
        "Period" => Key::Layout('.'),
        "Slash" => Key::Layout('/'),
        v if v.len() == 4 && v.starts_with("Key") && v.as_bytes()[3].is_ascii_uppercase() => {
            Key::Layout((v.as_bytes()[3] as char).to_ascii_lowercase())
        }
        v if v.len() == 6 && v.starts_with("Digit") && v.as_bytes()[5].is_ascii_digit() => {
            Key::Layout(v.as_bytes()[5] as char)
        }
        _ => bail!("暂不支持该按键：{value}"),
    })
}

fn validate_vp8(data: &[u8], width: u32, height: u32) -> Result<()> {
    ensure!(data.len() >= 3, "VP8 帧不完整");
    if data[0] & 1 == 0 {
        ensure!(
            data.len() >= 10 && data[3..6] == [0x9d, 0x01, 0x2a],
            "VP8 关键帧无效"
        );
        let encoded_width = (u16::from_le_bytes([data[6], data[7]]) & 0x3fff) as u32;
        let encoded_height = (u16::from_le_bytes([data[8], data[9]]) & 0x3fff) as u32;
        rc_desktop::validate_size(encoded_width, encoded_height)?;
        ensure!(
            encoded_width == width && encoded_height == height,
            "编码画面分辨率与消息不符"
        );
    }
    Ok(())
}

async fn self_test() -> Result<()> {
    // Deterministic codec round trip: no screen capture or input injection.
    let mut encoder = encoder(64, 64)?;
    let yuv = vec![128u8; 64 * 64 * 8];
    let video = encoder.encode_to_message(scrap::EncodeInput::YUV(&yuv), 0)?;
    let mut decoder = Decoder::new(CodecFormat::VP8, None);
    let mut rgb = ImageRgb::new(ImageFormat::ABGR, 1);
    let mut texture = ImageTexture::default();
    let ok = decoder.handle_video_frame(
        video.union.as_ref().context("编码器未输出画面")?,
        &mut rgb,
        &mut texture,
        &mut true,
        &mut None,
    )?;
    ensure!(
        ok && rgb.w == 64 && rgb.h == 64 && rgb.raw.len() == 16384,
        "VP8 编解码自检失败"
    );
    println!(
        "VP8 encode/decode passed: {}x{}, {} RGBA bytes",
        rgb.w,
        rgb.h,
        rgb.raw.len()
    );
    Ok(())
}
