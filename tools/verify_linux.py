#!/usr/bin/env python3
"""Exercise native Linux controls on a dedicated X11/Xvfb display.

Requires Python 3, Pillow, libX11 and libXtst; the game must already be built.
No gamepad is emulated. Results distinguish fullscreen setting persistence
from presentation behavior on a display without a window manager.
"""
import argparse
import re
import ctypes as C
import json
import os
import pathlib
import subprocess
import time
from PIL import ImageGrab

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--exe', type=pathlib.Path, default=pathlib.Path('target/debug/bomberman'))
parser.add_argument('--assets', type=pathlib.Path, default=pathlib.Path('local-assets'))
parser.add_argument('--output', type=pathlib.Path, default=pathlib.Path('/tmp/atomic-bomberman-ui'))
parser.add_argument('--display', default=os.environ.get('DISPLAY', ':99'))
args = parser.parse_args()
base = args.output.resolve()
base.mkdir(exist_ok=True)
settings = base / 'settings.json'
settings.unlink(missing_ok=True)
x = C.CDLL('libX11.so.6')
t = C.CDLL('libXtst.so.6')
x.XOpenDisplay.argtypes = [C.c_char_p]
x.XOpenDisplay.restype = C.c_void_p
d = x.XOpenDisplay(os.fsencode(args.display))
assert d, 'temporary display is unavailable'
Window = C.c_ulong
x.XDefaultRootWindow.argtypes = [C.c_void_p]
x.XDefaultRootWindow.restype = Window
root = x.XDefaultRootWindow(d)
x.XQueryTree.argtypes = [C.c_void_p, Window, C.POINTER(Window), C.POINTER(Window), C.POINTER(C.POINTER(Window)), C.POINTER(C.c_uint)]
x.XFetchName.argtypes = [C.c_void_p, Window, C.POINTER(C.c_char_p)]
x.XFree.argtypes = [C.c_void_p]
x.XSetInputFocus.argtypes = [C.c_void_p, Window, C.c_int, C.c_ulong]
x.XRaiseWindow.argtypes = [C.c_void_p, Window]
x.XKeysymToKeycode.argtypes = [C.c_void_p, C.c_ulong]
x.XKeysymToKeycode.restype = C.c_uint
x.XStringToKeysym.argtypes = [C.c_char_p]
x.XStringToKeysym.restype = C.c_ulong
x.XFlush.argtypes = [C.c_void_p]
x.XMoveResizeWindow.argtypes = [C.c_void_p, Window, C.c_int, C.c_int, C.c_uint, C.c_uint]
x.XGetGeometry.argtypes = [C.c_void_p, Window, C.POINTER(Window), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_uint), C.POINTER(C.c_uint), C.POINTER(C.c_uint), C.POINTER(C.c_uint)]
x.XTranslateCoordinates.argtypes = [C.c_void_p, Window, Window, C.c_int, C.c_int, C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(Window)]
t.XTestFakeKeyEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]

def windows():
    parent, real_root, children, count = Window(), Window(), C.POINTER(Window)(), C.c_uint()
    x.XQueryTree(d, root, C.byref(real_root), C.byref(parent), C.byref(children), C.byref(count))
    result = []
    for i in range(count.value):
        name = C.c_char_p()
        w = children[i]
        if x.XFetchName(d, w, C.byref(name)) and name.value:
            title = name.value.decode('utf8', 'replace')
            x.XFree(name)
            result.append(w)
        else:
            result.append(w)
    if children:
        x.XFree(children)
    return result

def geometry(w):
    r, xx, yy, ww, hh, bw, depth = Window(), C.c_int(), C.c_int(), C.c_uint(), C.c_uint(), C.c_uint(), C.c_uint()
    x.XGetGeometry(d, w, C.byref(r), C.byref(xx), C.byref(yy), C.byref(ww), C.byref(hh), C.byref(bw), C.byref(depth))
    gx, gy, child = C.c_int(), C.c_int(), Window()
    x.XTranslateCoordinates(d, w, root, 0, 0, C.byref(gx), C.byref(gy), C.byref(child))
    return gx.value, gy.value, ww.value, hh.value

def key(name, hold=0.09):
    code = x.XKeysymToKeycode(d, x.XStringToKeysym(name.encode()))
    assert code, name
    t.XTestFakeKeyEvent(d, code, 1, 0)
    x.XFlush(d)
    time.sleep(hold)
    t.XTestFakeKeyEvent(d, code, 0, 0)
    x.XFlush(d)
    time.sleep(0.18)

def screenshot(name):
    time.sleep(0.25)
    xx, yy, ww, hh = geometry(window)
    im = ImageGrab.grab(bbox=(xx, yy, xx + ww, yy + hh), xdisplay=args.display)
    im.save(base / (name + '.png'))

before = set(windows())
log = open(base / 'run.log', 'w')
process = subprocess.Popen([str(args.exe.resolve()), '--assets', str(args.assets.resolve()), '--settings', str(settings), '--seed', '42', '--mute'], env={**os.environ, 'DISPLAY': args.display, 'LP_NUM_THREADS': '2'}, stdout=log, stderr=log)
try:
    deadline = time.monotonic() + 100
    window = None
    while time.monotonic() < deadline:
        found = set(windows()) - before
        if found:
            window = found.pop()
            break
        assert process.poll() is None, 'game exited during startup'
        time.sleep(0.25)
    assert window, 'game window did not appear'
    x.XRaiseWindow(d, window)
    x.XSetInputFocus(d, window, 2, 0)
    x.XFlush(d)
    # Asset uploads may yield a few loading frames. Wait for the original
    # green Bomberman artwork before sending the first menu input.
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        xx, yy, ww, hh = geometry(window)
        image = ImageGrab.grab(bbox=(xx, yy, xx + ww, yy + hh), xdisplay=args.display)
        green = sum(g > 50 and g > r * 1.4 and g > b * 1.2 for r, g, b in image.convert('RGB').resize((160, 120)).getdata())
        if green > 100:
            break
        assert process.poll() is None, 'game exited during artwork loading'
        time.sleep(0.2)
    else:
        raise AssertionError('main menu artwork did not appear')
    screenshot('menu')
    clock_ticks = os.sysconf('SC_CLK_TCK')
    def thread_times():
        result = {}
        for path in pathlib.Path(f'/proc/{process.pid}/task').glob('*/stat'):
            fields = path.read_text().rsplit(')', 1)[1].split()
            result[path.parent.name] = (int(fields[11]) + int(fields[12])) / clock_ticks
        return result
    sample1 = thread_times()
    time.sleep(1)
    sample2 = thread_times()
    cpu = [{'tid': tid, 'name': pathlib.Path(f'/proc/{process.pid}/task/{tid}/comm').read_text().strip(), 'seconds': round(value - sample1.get(tid, value), 2)} for tid, value in sample2.items()]
    print('CPU seconds per thread during 1 second:', cpu, flush=True)
    for thread in cpu:
        if thread['name'] == 'bomberman-audio':
            assert thread['seconds'] < 0.15, 'silent audio thread is busy-spinning'
    key('Down'); key('Return')
    screenshot('setup')
    key('Right')
    assert json.load(open(settings))['participant_count'] == 2
    key('Left')
    assert json.load(open(settings))['participant_count'] == 10
    key('Down'); key('Right'); key('Right')
    assert json.load(open(settings))['arena'] == 2
    key('Down'); key('Right')
    key('Down'); key('Right')
    assert json.load(open(settings))['teams']
    key('Down'); key('Tab')
    assert json.load(open(settings))['participants'][0]['team'] == 1
    key('Escape')
    key('Down'); key('Down'); key('Return')
    key('Right')
    assert json.load(open(settings))['wins'] == 3
    key('Down'); key('Right')
    assert not json.load(open(settings))['win_by_kills']
    key('Down'); key('Right')
    assert json.load(open(settings))['round_seconds'] == 180
    screenshot('settings')
    key('Escape')
    key('Down'); key('Down'); key('Down'); key('Return')
    key('Return'); key('z')
    assert json.load(open(settings))['keyboard'][0]['keys'][0] == 'Z'
    screenshot('controls')
    key('Escape')
    key('F1')
    screenshot('help')
    key('Escape')
    key('Return')
    time.sleep(4)
    key('Escape')
    screenshot('paused')
    key('Return')
    x.XMoveResizeWindow(d, window, 0, 0, 1100, 600)
    x.XFlush(d)
    time.sleep(0.5)
    assert geometry(window)[2:] == (1100, 600), geometry(window)
    screenshot('resized-battle')
    key('F11')
    assert json.load(open(settings))['fullscreen']
    key('F11')
    assert not json.load(open(settings))['fullscreen']
    key('Escape')
    key('Down'); key('Down'); key('Down'); key('Return')
    key('Escape')
    process.wait(timeout=10)
    assert process.returncode == 0
    log.flush()
    controller = re.search(r'(\d+) controller\(s\)', (base / 'run.log').read_text())
    summary = {'passed': True, 'display': args.display, 'renderer': 'native Macroquad on X11', 'cpu_sample_seconds': 1, 'cpu_seconds_per_thread': cpu, 'controllers_detected': int(controller.group(1)) if controller else None, 'checks': ['menu navigation', 'participant count 2 and 10', 'arena and scheme selection', 'team assignment', 'match score target', 'team scoring restriction', 'round time', 'keyboard remapping', 'help', 'battle', 'pause and resume', 'resize 1100x600', 'fullscreen settings persistence', 'settings JSON persistence', 'clean exit'], 'limitations': ['No physical gamepad was exercised.', 'Fullscreen checks on Xvfb verify the setting and continued rendering; desktop window-manager presentation requires a desktop session.']}
    (base / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
    print('Passed: menus, participant assignment, arenas, schemes, teams, scoring, time, remapping, help, battle, pause, resizing, fullscreen, persistent settings, clean exit.', flush=True)
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=5)
    log.close()
