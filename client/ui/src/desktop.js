import { Channel, invoke } from "@tauri-apps/api/core";
import "./desktop.css";

export function makeDesktop({ title, control, close, notify }) {
  const container = document.createElement("section");
  container.className = "desktop-view";
  container.innerHTML = '<div class="desktop-toolbar"><span class="desktop-name"></span><span class="desktop-mode"></span><span class="desktop-size"></span><button class="secondary-button desktop-disconnect">断开</button></div><div class="desktop-stage"><canvas tabindex="0" aria-label="远程桌面"></canvas><div class="desktop-overlay" role="status">正在建立加密连接…</div></div><div class="desktop-footer"></div>';
  container.querySelector(".desktop-name").textContent = title;
  container.querySelector(".desktop-mode").textContent = control ? "允许控制" : "仅查看";
  container.querySelector(".desktop-disconnect").onclick = close;
  container.querySelector(".desktop-footer").textContent = control ? "点击画面后使用键盘和鼠标 · 移出画面会释放按键 · Ctrl + Alt + Escape 返回本机" : "仅查看画面 · 键盘和鼠标不会发送到远端";
  const canvas = container.querySelector("canvas");
  const overlay = container.querySelector(".desktop-overlay");
  const context = canvas.getContext("2d", { alpha: false });
  let session, disposed = false, connected = false, pendingAck;
  let sending = Promise.resolve();
  let inputCount = 0;
  const send = (event) => {
    if (!control || !connected || disposed || !session) return;
    // Keep press/release order through async IPC. Never accumulate unlimited input.
    if (++inputCount > 64) { notify("输入繁忙，已断开桌面以释放按键"); close(); return; }
    sending = sending.then(() => invoke("desktop_input", { session, event })).catch((e) => { if (!disposed) { notify(String(e)); close(); } }).finally(() => inputCount--);
  };
  const release = () => send({ type: "release_all" });
  const point = (e) => {
    const rect = canvas.getBoundingClientRect();
    return { type: "pointer", x: Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)), y: Math.max(0, Math.min(1, (e.clientY - rect.top) / rect.height)) };
  };
  let lastMove = 0;
  canvas.onpointermove = (e) => { if (performance.now() - lastMove < 30) return; lastMove = performance.now(); send(point(e)); };
  const button = (e, down) => {
    if (![0, 1, 2].includes(e.button)) return;
    e.preventDefault(); canvas.focus({ preventScroll: true });
    send(point(e)); send({ type: "button", button: ["left", "middle", "right"][e.button], down });
  };
  canvas.onpointerdown = (e) => button(e, true);
  canvas.onpointerup = (e) => button(e, false);
  canvas.onpointerleave = release;
  canvas.onpointercancel = release;
  canvas.onblur = release;
  canvas.oncontextmenu = (e) => e.preventDefault();
  canvas.addEventListener("wheel", (e) => { if (!control) return; e.preventDefault(); send({ type: "scroll", x: Math.sign(e.deltaX) * Math.min(10, Math.ceil(Math.abs(e.deltaX) / 50)), y: Math.sign(e.deltaY) * Math.min(10, Math.ceil(Math.abs(e.deltaY) / 50)) }); }, { passive: false });
  const supported = /^(Key[A-Z]|Digit[0-9]|F([1-9]|1[012])|Control(Left|Right)|Shift(Left|Right)|Alt(Left|Right)|Meta(Left|Right)|Enter|NumpadEnter|Escape|Tab|Backspace|Delete|Space|Arrow(Up|Down|Left|Right)|Home|End|PageUp|PageDown|Minus|Equal|BracketLeft|BracketRight|Backslash|Semicolon|Quote|Backquote|Comma|Period|Slash)$/;
  const key = (e, down) => {
    if (!control || e.isComposing) return;
    if (e.code === "Escape" && e.ctrlKey && e.altKey) { e.preventDefault(); release(); canvas.blur(); container.querySelector("button").focus(); return; }
    if (!supported.test(e.code)) return;
    e.preventDefault(); e.stopPropagation(); send({ type: "key", key: e.code, down });
  };
  canvas.onkeydown = (e) => key(e, true);
  canvas.onkeyup = (e) => key(e, false);
  canvas.oncompositionend = (e) => { if (e.data) send({ type: "text", text: e.data }); };
  window.addEventListener("blur", release);
  const acknowledge = (sequence) => {
    if (!session) { pendingAck = sequence; return; }
    invoke("desktop_ack", { session, sequence }).catch(() => {});
  };
  const frames = new Channel();
  frames.onmessage = (buffer) => {
    if (disposed) return;
    try {
      const data = buffer instanceof ArrayBuffer ? buffer : Uint8Array.from(buffer).buffer;
      const header = new DataView(data);
      const sequence = Number(header.getBigUint64(0));
      const width = header.getUint32(8), height = header.getUint32(12);
      if (width * height > 16 * 1024 * 1024 || data.byteLength !== 16 + width * height * 4) throw new Error("画面数据无效");
      if (canvas.width !== width || canvas.height !== height) { canvas.width = width; canvas.height = height; }
      context.putImageData(new ImageData(new Uint8ClampedArray(data, 16), width, height), 0, 0);
      overlay.hidden = true;
      container.querySelector(".desktop-size").textContent = `${width} × ${height}`;
      acknowledge(sequence);
    } catch (error) { notify(error.message); close(); }
  };
  const status = new Channel();
  status.onmessage = (value) => {
    if (disposed) return;
    connected = value.phase === "connected";
    if (connected) overlay.textContent = "已连接，正在接收画面…";
    else { overlay.textContent = value.reason || "桌面已断开"; overlay.hidden = false; }
  };
  return {
    container,
    async connect(request) {
      try {
        session = await invoke("desktop_connect", { request, frames, status });
        if (disposed) { await invoke("desktop_close", { session }); return; }
        if (pendingAck !== undefined) acknowledge(pendingAck);
      } catch (error) { overlay.textContent = String(error); overlay.hidden = false; }
    },
    hide() { release(); container.hidden = true; },
    dispose() { release(); disposed = true; window.removeEventListener("blur", release); if (session) invoke("desktop_close", { session }).catch(() => {}); container.remove(); },
  };
}
