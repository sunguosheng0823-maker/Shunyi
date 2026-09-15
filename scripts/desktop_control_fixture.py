#!/usr/bin/env python3
"""An input receiver for isolated Windows/Linux CI desktops. Never launch on a user's desktop without consent."""
import ctypes
import json
import os
from pathlib import Path
import sys
import time
import tkinter as tk

root_path = Path(sys.argv[1])
if sys.platform == "win32":
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
window = tk.Tk()
window.title("Shunyi isolated desktop acceptance")
window.geometry("720x420+40+40")
window.configure(bg="#10213a")
window.attributes("-topmost", True)
canvas = tk.Canvas(window, width=720, height=420, bg="#10213a", highlightthickness=0)
canvas.pack(fill="both", expand=True)
canvas.create_rectangle(25, 25, 200, 170, fill="#08b8a0", outline="")
canvas.create_rectangle(225, 25, 400, 170, fill="#e5a633", outline="")
canvas.create_rectangle(425, 25, 600, 170, fill="#8875de", outline="")
canvas.create_text(30, 220, anchor="w", text="Shunyi native capture and input test", fill="white", font=("Arial", 22))
canvas.create_text(30, 260, anchor="w", text="This window belongs to the disposable CI test.", fill="#d0dbe9", font=("Arial", 14))
events = (root_path / "control-events.jsonl").open("w", buffering=1)

def record(kind, event):
    key = getattr(event, "keysym", "").lower()
    if key not in ("a", "b"):
        key = getattr(event, "char", "").lower()
    code = {"a": 0, "b": 11}.get(key, -1) if kind.startswith("key_") else 0
    events.write(json.dumps({"event": kind, "code": code, "keysym": getattr(event, "keysym", ""), "time": time.time()}) + "\n")

window.bind("<ButtonPress-1>", lambda e: record("mouse_down", e))
window.bind("<ButtonRelease-1>", lambda e: record("mouse_up", e))
window.bind("<KeyPress>", lambda e: record("key_down", e))
window.bind("<KeyRelease>", lambda e: record("key_up", e))

def ready():
    window.lift()
    window.focus_force()
    window.update_idletasks()
    pid = os.getpid()
    foreground = pid if window.focus_displayof() is not None else -1
    if sys.platform == "win32":
        ctypes.windll.user32.GetForegroundWindow.restype = ctypes.c_void_p
        ctypes.windll.user32.GetWindowThreadProcessId.argtypes = [ctypes.c_void_p, ctypes.POINTER(ctypes.c_ulong)]
        owner = ctypes.c_ulong()
        ctypes.windll.user32.GetWindowThreadProcessId(ctypes.windll.user32.GetForegroundWindow(), ctypes.byref(owner))
        foreground = owner.value
    target = {"x": window.winfo_rootx() + 360, "y": window.winfo_rooty() + 310, "pid": pid, "front_pid": foreground}
    (root_path / "control-target.json").write_text(json.dumps(target))

window.after(600, ready)
window.mainloop()
