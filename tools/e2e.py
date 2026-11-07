"""End-to-end test of the real exe against a local mock API, plus optional real CLI runs.
Runs from an isolated portable folder. Usage: python tools/e2e.py <work_dir> [--cli]"""
import base64, ctypes, http.server, json, os, shutil, struct, subprocess, sys, threading, time, zlib
from ctypes import wintypes as wt

u = ctypes.WinDLL("user32", use_last_error=True)
u.SetProcessDPIAware()
u.SendMessageW.restype = ctypes.c_ssize_t
u.SendMessageW.argtypes = [wt.HWND, wt.UINT, ctypes.c_size_t, ctypes.c_void_p]
u.PostMessageW.argtypes = [wt.HWND, wt.UINT, ctypes.c_size_t, ctypes.c_ssize_t]
WM_SETTEXT, WM_GETTEXT, WM_GETTEXTLENGTH, WM_KEYDOWN, BM_CLICK = 0x0C, 0x0D, 0x0E, 0x100, 0xF5
WM_TOGGLE = 0x8000 + 2
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "release", "LitePilot.exe")
work = os.path.abspath(sys.argv[1])
results = []
seen = {"auth": None, "body": None}


def check(name, ok, detail=""):
    results.append(ok)
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""), flush=True)


def png():
    raw = b"".join(b"\0" + bytes([200, 60, 60]) * 48 for _ in range(48))
    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 48, 48, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


REPLY = ("# Heading\n\nSome **bold**, *italic*, `inline` and a [link](https://example.com).\n\n"
         "- first\n- second\n\n| Name | Value |\n|---|---|\n| alpha | 1 |\n| beta | 2 |\n\n"
         "```rust\n# a comment\nfn main() { println!(\"{}\", 1); }\n```\n\nمرحبا بالعالم\n\n"
         f"![dot](data:image/png;base64,{base64.b64encode(png()).decode()})\n\n" + "".join(f"filler {i}\n\n" for i in range(40)) + "ENDMARK\n")


class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def log_message(self, *a): pass
    def do_POST(self):
        body = self.rfile.read(int(self.headers["Content-Length"]))
        seen["auth"], seen["body"] = self.headers.get("Authorization"), json.loads(body)
        last = seen["body"]["messages"][-1]["content"]
        if "fail401" in last:
            d = b'{"error":{"message":"bad key"}}'
            self.send_response(401); self.send_header("Content-Length", str(len(d))); self.end_headers(); self.wfile.write(d); return
        self.send_response(200); self.send_header("Content-Type", "text/event-stream"); self.send_header("Connection", "close"); self.end_headers()
        slow = "slow" in last
        text = "tick " * 400 if slow else REPLY
        size = 5 if slow else 40
        try:
            for i in range(0, len(text), size):
                self.wfile.write(("data: " + json.dumps({"choices": [{"delta": {"content": text[i:i+size]}}]}) + "\n\n").encode()); self.wfile.flush()
                time.sleep(0.05 if slow else 0.002)
            self.wfile.write(b"data: [DONE]\n\n"); self.wfile.flush()
        except OSError:
            seen["aborted"] = True
        self.close_connection = True


def ps_mem(pid):
    out = subprocess.run(["powershell", "-NoProfile", "-Command", f"$p=Get-Process -Id {pid};\"$($p.WorkingSet64) $($p.PrivateMemorySize64) $($p.TotalProcessorTime.TotalMilliseconds) $($p.Threads.Count)\""], capture_output=True, text=True).stdout.split()
    return int(out[0]) / 1048576, int(out[1]) / 1048576, float(out[2].replace(",", ".")), int(out[3])


def children(cls_parent):
    h = []
    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(c, _):
        b = ctypes.create_unicode_buffer(64); u.GetClassNameW(c, b, 64); h.append((b.value, c)); return True
    u.EnumChildWindows(cls_parent, cb, 0)
    return h


def get_text(h):
    n = u.SendMessageW(h, WM_GETTEXTLENGTH, 0, None)
    b = ctypes.create_unicode_buffer(n + 2)
    u.SendMessageW(h, WM_GETTEXT, n + 1, ctypes.addressof(b))
    return b.value


def set_text(h, s):
    b = ctypes.create_unicode_buffer(s)
    u.SendMessageW(h, WM_SETTEXT, 0, ctypes.addressof(b))


def wait(fn, t=15):
    end = time.time() + t
    while time.time() < end:
        if fn():
            return True
        time.sleep(0.02)
    return False


def write_cfg(active, extra=None):
    os.makedirs(os.path.join(work, "LitePilotData"), exist_ok=True)
    profs = [{"id": "mock", "name": "Mock API", "kind": "openai", "base_url": "http://127.0.0.1:18765/v1", "model": "m1"}]
    profs += [{"id": "codex", "name": "ChatGPT", "kind": "codex", "base_url": "", "model": "gpt-5.6-luna", "effort": "low"}, {"id": "claude", "name": "Claude", "kind": "claude", "base_url": "", "model": "haiku", "effort": "low"}]
    json.dump({"active": active, "hotkey": "Ctrl+Alt+Shift+F11", "profiles": profs}, open(os.path.join(work, "LitePilotData", "config.json"), "w"))


def launch():
    p = subprocess.Popen([os.path.join(work, "LitePilot.exe")])
    assert wait(lambda: u.FindWindowW("LitePilotMain", None)), "window"
    main = u.FindWindowW("LitePilotMain", None)
    wait(lambda: sum(n.lower().startswith("richedit") for n, _ in children(main)) == 2)
    time.sleep(.2)
    cs = children(main)
    edits = [c for n, c in cs if n.lower().startswith("richedit")]
    return p, main, edits[0], edits[1], u.GetDlgItem(main, 10)


def send(main, inp, text):
    set_text(inp, text)
    u.PostMessageW(inp, WM_KEYDOWN, 0x0D, 0)


def quit_app(p, main):
    u.PostMessageW(main, 0x8005, 0, 0)
    p.wait(5)


def screenshot(path):
    script = os.path.join(work, "shot.ps1")
    open(script, "w").write("""Add-Type -Name D -Namespace W -MemberDefinition '[DllImport("user32")] public static extern bool SetProcessDPIAware();'
[W.D]::SetProcessDPIAware() | Out-Null
Add-Type -A System.Windows.Forms,System.Drawing
$b=[Windows.Forms.Screen]::PrimaryScreen.Bounds;$i=New-Object Drawing.Bitmap $b.Width,$b.Height
[Drawing.Graphics]::FromImage($i).CopyFromScreen(0,0,0,0,$i.Size);$i.Save($args[0])
$n=0;$m=99999;for($x=0;$x -lt $b.Width;$x+=4){for($y=0;$y -lt $b.Height;$y+=4){$p=$i.GetPixel($x,$y);if($p.R -eq 200 -and $p.G -eq 60 -and $p.B -eq 60){$n++;if($x -lt $m){$m=$x}}}}
"$n $m" | Out-File -Encoding ascii ($args[0]+".red")
""")
    subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, path])


shutil.rmtree(work, ignore_errors=True)
os.makedirs(work)
shutil.copy(EXE, work)
open(os.path.join(work, "LitePilot.portable"), "w").close()
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 18765), H)
threading.Thread(target=srv.serve_forever, daemon=True).start()

write_cfg("mock")
p, main, view, inp, btn = launch()
time.sleep(3)
ws, priv, cpu0, th = ps_mem(p.pid)
print(f"METRIC idle-after-launch working_set={ws:.1f}MB private={priv:.1f}MB threads={th}")

lat = []
for _ in range(10):
    t = time.perf_counter(); u.PostMessageW(main, WM_TOGGLE, 0, 0)
    wait(lambda: u.IsWindowVisible(main), 2); lat.append((time.perf_counter() - t) * 1000)
    time.sleep(.1); u.PostMessageW(main, WM_TOGGLE, 0, 0); wait(lambda: not u.IsWindowVisible(main), 2); time.sleep(.1)
print(f"METRIC toggle-to-visible ms min={min(lat):.1f} max={max(lat):.1f} (cold first={lat[0]:.1f})")

def press(*keys):
    for k in keys: u.keybd_event(k, 0, 0, 0)
    for k in reversed(keys): u.keybd_event(k, 0, 2, 0)
press(0x11, 0x12, 0x10, 0x7A)  # the configured shortcut
check("configured shortcut opens the app", wait(lambda: u.IsWindowVisible(main), 3))
if not u.IsWindowVisible(main): u.PostMessageW(main, WM_TOGGLE, 0, 0); wait(lambda: u.IsWindowVisible(main))
check("input has keyboard focus", wait(lambda: u.GetForegroundWindow() == main, 2))
u.PostMessageW(btn, BM_CLICK, 0, 0)
check("settings opens", wait(lambda: u.IsWindowVisible(u.FindWindowW("LitePilotSettings", None))))
st = u.FindWindowW("LitePilotSettings", None)
check("welcome shows three cards", all(u.GetDlgItem(st, i) for i in (1101, 1102, 1103)))
time.sleep(.2)
screenshot(os.path.join(work, "settings-home.png"))
u.PostMessageW(st, 0x111, 1102, 0)
check("Claude card opens its page", wait(lambda: u.GetDlgItem(st, 1014), 3))
wait(lambda: "Checking" not in get_text(u.GetDlgItem(st, 1017)), 15)
active_now = json.load(open(os.path.join(work, "LitePilotData", "config.json")))["active"]
check("viewing a card doesn't switch the account", active_now == "mock", active_now)
time.sleep(.3)
screenshot(os.path.join(work, "settings-login.png"))
u.PostMessageW(st, 0x111, 1018, 0)
check("Back returns to the cards", wait(lambda: u.GetDlgItem(st, 1101), 3))
u.PostMessageW(st, 0x111, 1104, 0)
wait(lambda: u.GetDlgItem(st, 1008), 3)
time.sleep(.3)
screenshot(os.path.join(work, "settings-options.png"))
u.PostMessageW(st, 0x111, 1018, 0)
wait(lambda: u.GetDlgItem(st, 1101), 3)
cfg_now = lambda: json.load(open(os.path.join(work, "LitePilotData", "config.json")))
u.PostMessageW(st, 0x111, 1105, 0)
check("Appearance opens", wait(lambda: u.GetDlgItem(st, 1210), 3))
u.PostMessageW(st, 0x111, 1203, 0)
check("theme choice saves", wait(lambda: cfg_now().get("theme") == "light", 3))
u.PostMessageW(st, 0x111, 1213, 0)
check("text size choice saves", wait(lambda: cfg_now().get("text_size") == "large", 3))
time.sleep(.4)
screenshot(os.path.join(work, "appearance-light.png"))
CB_SETCURSEL, CBN = 0x14E, 1 << 16
pal = u.GetDlgItem(st, 1221)
u.SendMessageW(pal, CB_SETCURSEL, 6, None)
u.PostMessageW(st, 0x111, CBN | 1221, 0)
check("Custom colors open", wait(lambda: u.GetDlgItem(st, 1222), 3))
set_text(u.GetDlgItem(st, 1222), "not-a-color")
u.PostMessageW(st, 0x111, 1225, 0)
check("bad colors are refused", wait(lambda: "like #" in get_text(u.GetDlgItem(st, 1017)), 3))
set_text(u.GetDlgItem(st, 1222), "#301030")
u.PostMessageW(st, 0x111, 1225, 0)
check("custom colors save", wait(lambda: cfg_now().get("custom_bg") == "#301030", 3),
      f"field={get_text(u.GetDlgItem(st, 1222))!r} status={get_text(u.GetDlgItem(st, 1017))!r} saved={cfg_now().get('custom_bg')!r} palette={cfg_now().get('palette')!r}")
time.sleep(.4)
screenshot(os.path.join(work, "appearance-custom.png"))
u.SendMessageW(u.GetDlgItem(st, 1221), CB_SETCURSEL, 0, None)
u.PostMessageW(st, 0x111, CBN | 1221, 0)
wait(lambda: cfg_now().get("palette") == "", 3)
u.PostMessageW(st, 0x111, 1202, 0)
wait(lambda: cfg_now().get("theme") == "dark", 3)
u.PostMessageW(st, 0x111, 1212, 0)
wait(lambda: cfg_now().get("text_size") == "medium", 3)
time.sleep(.4)
screenshot(os.path.join(work, "appearance-dark.png"))
u.PostMessageW(st, 0x111, 1201, 0)
wait(lambda: cfg_now().get("theme") == "system", 3)
u.PostMessageW(st, 0x111, 1018, 0)
wait(lambda: u.GetDlgItem(st, 1101), 3)
u.PostMessageW(st, 0x111, 1103, 0)
check("API card opens its page", wait(lambda: u.GetDlgItem(st, 1006), 3))
time.sleep(.3)
screenshot(os.path.join(work, "settings-api.png"))
GetDlg = u.GetDlgItem
set_text(GetDlg(st, 1006), "e2e-test-key")
u.PostMessageW(st, 0x111, 1016, 0)
check("settings saves", wait(lambda: get_text(GetDlg(st, 1017)) == "Saved."), get_text(GetDlg(st, 1017)))
u.PostMessageW(st, 0x111, 2, 0)
wait(lambda: not u.FindWindowW("LitePilotSettings", None))
u.SetForegroundWindow(main)

t = time.perf_counter()
send(main, inp, "hello test")
check("markdown reply streams", wait(lambda: "ENDMARK" in get_text(view)))
print(f"METRIC mock-reply-rendered ms={(time.perf_counter()-t)*1000:.0f}")
check("key from Credential Manager is sent", seen["auth"] == "Bearer e2e-test-key", str(seen["auth"]))
check("settings profile used", seen["body"]["model"] == "m1")
txt = get_text(view)
check("input cleared", get_text(inp) == "")
for frag in ["Heading", "bold", "alpha", "beta", "println!", "first"]:
    check(f"rendered: {frag}", frag in txt)
check("no raw image data", "base64" not in txt and "[Image" not in txt)
check("no raw markdown", "**" not in txt and "```" not in txt and "|---" not in txt)
time.sleep(.3)
def red(name):
    screenshot(os.path.join(work, name))
    try:
        return [int(x) for x in open(os.path.join(work, name + ".red")).read().split()]
    except (OSError, ValueError):
        print(f"NOTE screenshot {name} failed (is the PC locked?)")
        return [-1, -1]
n, _ = red("bottom.png")
check("image is off screen when scrolled away", n == 0, str(n))
u.SendMessageW(view, 0x115, 6, None)
time.sleep(.3)
n, left = red("reply.png")
check("image is drawn when scrolled to", n > 20, str(n))
r = wt.RECT(); u.GetWindowRect(main, ctypes.byref(r))
check("image is at the left of the transcript", left - r.left < (r.right - r.left) // 5, f"{left - r.left}px of {r.right - r.left}")
u.SendMessageW(view, 0x115, 7, None)
time.sleep(.3)
print("METRIC visible-with-reply working_set=%.1fMB private=%.1fMB" % ps_mem(p.pid)[:2])

# Popup chrome: pin, drag area, prompt, hover, and tooltip.
GWL_EXSTYLE, TOPMOST = -20, 0x8
top = lambda: bool(u.GetWindowLongW(main, GWL_EXSTYLE) & TOPMOST)
pin_state = lambda: f"exstyle={u.GetWindowLongW(main, GWL_EXSTYLE) & 0xFFFFFFFF:#x} visible={u.IsWindowVisible(main)} cfg_on_top={cfg_now().get('on_top')!r} fg_is_popup={u.GetForegroundWindow() == main}"
check("keep on top is on by default", top(), pin_state())
u.PostMessageW(main, 0x111, 13, 0)
check("keep on top turns off", wait(lambda: not top(), 2), pin_state())
u.PostMessageW(main, 0x111, 13, 0)
check("keep on top turns back on", wait(top, 2), pin_state())
r = wt.RECT(); u.GetWindowRect(main, ctypes.byref(r))
u.DefWindowProcW.restype = ctypes.c_ssize_t
hit = u.SendMessageW(main, 0x84, 0, ctypes.c_void_p(((r.top + 12) << 16) | ((r.left + r.right) // 2)))
check("window background drags the window", hit == 2, str(hit))
for ch in "hi":
    u.PostMessageW(inp, 0x102, ord(ch), 0)
time.sleep(.3)
g = wt.RECT(); u.GetWindowRect(btn, ctypes.byref(g))
was = wt.POINT(); u.GetCursorPos(ctypes.byref(was))
u.SetCursorPos((g.left + g.right) // 2, (g.top + g.bottom) // 2)
time.sleep(1.2)
screenshot(os.path.join(work, "chrome.png"))
u.SetCursorPos(was.x, was.y)
set_text(inp, "")
time.sleep(.2)
screenshot(os.path.join(work, "chrome-empty.png"))
# Copy link on the code card, by a real click. The clipboard's text is restored afterwards.
class POINTL(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]
def read_clip():
    r = subprocess.run(["powershell", "-NoProfile", "-STA", "-Command", "Get-Clipboard -Raw"], capture_output=True, text=True, encoding="utf8", errors="replace")
    return r.stdout.rstrip("\r\n")
saved_clip = read_clip()
u.SendMessageW(view, 0x115, 6, None)
time.sleep(.3)
shot = os.path.join(work, "copy.png")
screenshot(shot)
r = wt.RECT(); u.GetWindowRect(view, ctypes.byref(r))
scan = os.path.join(work, "bars.ps1")
open(scan, "w").write("""Add-Type -A System.Drawing
$i=New-Object Drawing.Bitmap $args[0];$x=[int]$args[1];$t=[int]$args[2];$b=[int]$args[3];$l=[int]$args[4];$on=$false;$n=0
for($y=$t;$y -lt $b;$y++){$p=$i.GetPixel($x,$y);$m=($p.R -eq 60 -and $p.G -eq 73 -and $p.B -eq 85)
 if($m -and -not $on){$s=$y};if(-not $m -and $on -and ($y-$s) -ge 20){$n++;if($n -eq 2){$c=[int](($s+$y)/2);$e=$x;while($e -lt $l){$q=$i.GetPixel($e,$c);if($q.R -eq 60 -and $q.G -eq 73){$last=$e};$e++};"$last $c";exit}};$on=$m}
""")
out = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", scan, shot, str((r.left + r.right) // 2), str(r.top), str(r.bottom), str(r.right)], capture_output=True, text=True).stdout.split()
fg = u.GetForegroundWindow()
check("code card bar found on screen", len(out) == 2, f"scan={out!r} popup_in_front={fg == main} on_top={top()}")
if len(out) == 2:
    bx, by = int(out[0]), int(out[1])
    scale = u.GetDpiForWindow(main) / 96
    xy = ((by - r.top) << 16) | (bx - r.left - int(18 * scale))
    u.PostMessageW(view, 0x200, 0, xy)
    u.PostMessageW(view, 0x201, 1, xy)
    u.PostMessageW(view, 0x202, 0, xy)
    time.sleep(.3)
    clip = read_clip()
    check("header confirms the copy", get_text(u.GetDlgItem(main, 11)) == "Copied", get_text(u.GetDlgItem(main, 11)))
    check("app survives the click", p.poll() is None, str(p.poll()))
    check("Copy puts the code on the clipboard", "fn main()" in clip and "# a comment" in clip and "```" not in clip, repr(clip[:60]))
u.SendMessageW(view, 0x115, 7, None)
subprocess.run(["powershell", "-NoProfile", "-STA", "-Command", "Set-Clipboard -Value $env:LP_CLIP"], env={**os.environ, "LP_CLIP": saved_clip})
# Previous chats, driven by keys the way a keyboard user would.
u.PostMessageW(main, 0x111, 14, 0)
check("previous chats opens", wait(lambda: u.GetDlgItem(main, 21), 3))
hist_list = u.GetDlgItem(main, 21)
time.sleep(.3)
screenshot(os.path.join(work, "history.png"))
check("chat is hidden behind it", not u.IsWindowVisible(view))
search = u.GetDlgItem(main, 20)
n = lambda: u.SendMessageW(hist_list, 0x18B, 0, None)
check("lists the saved chat", n() == 1, str(n()))
u.PostMessageW(search, 0x100, 0x1B, 0)
check("Esc closes it", wait(lambda: not u.GetDlgItem(main, 21), 3) and u.IsWindowVisible(view))
press(0x11, 0x48)  # real Ctrl+H
check("Ctrl+H opens it", wait(lambda: u.GetDlgItem(main, 21), 3))
press(0x0D)
check("Enter opens the chosen chat", wait(lambda: not u.GetDlgItem(main, 21) and "ENDMARK" in get_text(view), 3))
send(main, inp, "fail401")
check("HTTP errors are shown", wait(lambda: "HTTP 401: bad key" in get_text(view)))
send(main, inp, "follow up")
time.sleep(1)
roles = [m["role"] for m in seen["body"]["messages"]]
check("error text isn't sent to the model", roles == ["user", "assistant", "user"] and seen["body"]["messages"][-1]["content"] == "follow up", str(roles))

wait(lambda: get_text(view).count("ENDMARK") >= 2)
send(main, inp, "slow one")
check("slow reply starts", wait(lambda: "tick tick" in get_text(view)))
u.PostMessageW(inp, WM_KEYDOWN, 0x1B, 0)
time.sleep(.3)
a = get_text(view); time.sleep(.6)
check("Esc stops the reply", a == get_text(view) and u.IsWindowVisible(main))
wait(lambda: seen.get("aborted"), 25)
check("server connection dropped on stop", seen.get("aborted") is True)

u.PostMessageW(inp, WM_KEYDOWN, 0x1B, 0)
check("Esc hides when idle", wait(lambda: not u.IsWindowVisible(main)))
time.sleep(1.5)
ws, priv, cpu1, th = ps_mem(p.pid)
print(f"METRIC idle-after-use working_set={ws:.1f}MB private={priv:.1f}MB threads={th}")
time.sleep(20)
ws, priv, cpu2, th = ps_mem(p.pid)
print(f"METRIC idle-cpu ms_over_20s={cpu2-cpu1:.1f} working_set={ws:.1f}MB")
quit_app(p, main)
hist = os.path.join(work, "LitePilotData", "chats")
check("history saved", len(os.listdir(hist)) == 1 and "ENDMARK" in open(os.path.join(hist, os.listdir(hist)[0]), encoding="utf8").read())
check("no key in settings file", "e2e-test-key" not in open(os.path.join(work, "LitePilotData", "config.json")).read())
write_cfg("mock")
p, main, view, inp, btn = launch()
u.PostMessageW(main, WM_TOGGLE, 0, 0)
check("history reloads after restart", wait(lambda: "ENDMARK" in get_text(view)))
quit_app(p, main)
subprocess.run(["cmdkey", "/delete:LitePilot/mock"], capture_output=True)

if "--cli" in sys.argv:
    for kind, expect in (("claude", "pong"), ("codex", "pong")):
        shutil.rmtree(os.path.join(work, "LitePilotData"), ignore_errors=True)
        write_cfg(kind)
        p, main, view, inp, btn = launch()
        u.PostMessageW(main, WM_TOGGLE, 0, 0); wait(lambda: u.IsWindowVisible(main))
        t = time.perf_counter()
        send(main, inp, "Reply with the single lowercase word: pong")
        peak, stop, cpid = [0.0], [False], [None]
        def sample():
            while not stop[0]:
                r = subprocess.run(["powershell", "-NoProfile", "-Command", f"(Get-CimInstance Win32_Process -Filter 'ParentProcessId={p.pid}' | Measure-Object WorkingSetSize -Sum).Sum"], capture_output=True, text=True).stdout.strip()
                if r: peak[0] = max(peak[0], float(r) / 1048576)
        th = threading.Thread(target=sample); th.start()
        def finished():
            t = get_text(view)
            return t.count(expect) >= 2 or "Error" in t
        done = wait(finished, 120)
        dt = time.perf_counter() - t
        stop[0] = True; th.join()
        out = get_text(view)
        print(f"METRIC {kind} first-reply s={dt:.1f} cli_peak_working_set={peak[0]:.0f}MB")
        print(f"OUTPUT {kind}: {out.strip()[:300]!r}")
        check(f"{kind} replied through LitePilot", done and "Error" not in out)
        time.sleep(1.5)
        left = subprocess.run(["powershell", "-NoProfile", "-Command", f"@(Get-CimInstance Win32_Process -Filter 'ParentProcessId={p.pid}').Count"], capture_output=True, text=True).stdout.strip()
        check(f"{kind} CLI exited after reply", left == "0", left)
        quit_app(p, main)

subprocess.run(["cmdkey", "/delete:LitePilot/mock"], capture_output=True)
print(f"\n{sum(results)}/{len(results)} checks passed")
sys.exit(0 if all(results) else 1)
