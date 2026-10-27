#!/usr/bin/env python3
"""Verify launch overrides, X11 window resizing and F11 on an owned display.

Requires Python 3, Pillow, libX11/libXtst, a built game and an imported cache.
Fullscreen property changes are checked on Xvfb; no window manager is assumed.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import time
from PIL import ImageGrab

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--exe', type=Path, default=Path('target/debug/bomberman'))
parser.add_argument('--assets', type=Path, default=Path('local-assets'))
parser.add_argument('--display', default=os.environ.get('DISPLAY', ':99'))
parser.add_argument('--output', type=Path, default=Path('/tmp/atomic-bomberman-windowed'))
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=True)
x, xt = C.CDLL('libX11.so.6'), C.CDLL('libXtst.so.6')
W, P = C.c_ulong, C.c_void_p
x.XOpenDisplay.argtypes, x.XOpenDisplay.restype = [C.c_char_p], P
x.XDefaultRootWindow.argtypes, x.XDefaultRootWindow.restype = [P], W
x.XQueryTree.argtypes = [P,W,C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(W)),C.POINTER(C.c_uint)]
x.XFree.argtypes = [P]
x.XFetchName.argtypes = [P,W,C.POINTER(C.c_char_p)]
x.XGetGeometry.argtypes = [P,W,C.POINTER(W),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint)]
x.XMoveResizeWindow.argtypes = [P,W,C.c_int,C.c_int,C.c_uint,C.c_uint]
x.XSetInputFocus.argtypes = [P,W,C.c_int,W]
x.XFlush.argtypes = [P]
x.XInternAtom.argtypes, x.XInternAtom.restype = [P,C.c_char_p,C.c_int], W
x.XGetWindowProperty.argtypes = [P,W,W,C.c_long,C.c_long,C.c_int,W,C.POINTER(W),C.POINTER(C.c_int),C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(C.c_ubyte))]
x.XStringToKeysym.argtypes, x.XStringToKeysym.restype = [C.c_char_p], W
x.XKeysymToKeycode.argtypes, x.XKeysymToKeycode.restype = [P,W], C.c_uint
xt.XTestFakeKeyEvent.argtypes = [P,C.c_uint,C.c_int,W]
d = x.XOpenDisplay(os.fsencode(args.display))
assert d, 'owned X11 display is unavailable'
root = x.XDefaultRootWindow(d)
state_atom = x.XInternAtom(d,b'_NET_WM_STATE',0)
fullscreen_atom = x.XInternAtom(d,b'_NET_WM_STATE_FULLSCREEN',0)

def windows():
    rr, parent, count, children = W(),W(),C.c_uint(),C.POINTER(W)()
    x.XQueryTree(d,root,C.byref(rr),C.byref(parent),C.byref(children),C.byref(count))
    # The display belongs to this test. Miniquad sets _NET_WM_NAME rather
    # than legacy WM_NAME, so detect newly created top-level windows directly.
    result = {children[i] for i in range(count.value)}
    if children: x.XFree(children)
    return result

def geometry(window):
    r,xx,yy,ww,hh,bw,depth = W(),C.c_int(),C.c_int(),C.c_uint(),C.c_uint(),C.c_uint(),C.c_uint()
    x.XGetGeometry(d,window,C.byref(r),C.byref(xx),C.byref(yy),C.byref(ww),C.byref(hh),C.byref(bw),C.byref(depth))
    return xx.value,yy.value,ww.value,hh.value

def fullscreen(window):
    kind,fmt,count,remaining,data = W(),C.c_int(),W(),W(),C.POINTER(C.c_ubyte)()
    x.XGetWindowProperty(d,window,state_atom,0,1024,0,0,C.byref(kind),C.byref(fmt),C.byref(count),C.byref(remaining),C.byref(data))
    result = fmt.value == 32 and fullscreen_atom in [C.cast(data,C.POINTER(W))[i] for i in range(count.value)]
    if data: x.XFree(data)
    return result

def key(name):
    code = x.XKeysymToKeycode(d,x.XStringToKeysym(name.encode()))
    xt.XTestFakeKeyEvent(d,code,1,0); x.XFlush(d); time.sleep(.12)
    xt.XTestFakeKeyEvent(d,code,0,0); x.XFlush(d); time.sleep(.35)

def wait_until(predicate, timeout=100):
    deadline = time.monotonic()+timeout
    while time.monotonic()<deadline:
        value = predicate()
        if value: return value
        assert process.poll() is None, 'game exited during verification'
        time.sleep(.2)
    raise AssertionError('timed out waiting for native window state')

checks=[]
for name, saved, flags, expected in [
    ('windowed-overrides-saved-fullscreen', True, ['--fullscreen','--windowed'], False),
    ('fullscreen-last-flag', False, ['--windowed','--fullscreen'], True),
    ('saved-windowed-without-override', False, [], False),
]:
    settings = args.output/(name+'.json')
    settings.write_text(json.dumps({'fullscreen':saved}))
    before=windows()
    with open(args.output/(name+'.log'),'w') as log:
        process=subprocess.Popen([str(args.exe.resolve()),'--assets',str(args.assets.resolve()),'--settings',str(settings),'--mute',*flags], stdout=log,stderr=log,env={**os.environ,'DISPLAY':args.display,'LP_NUM_THREADS':'2'})
        try:
            window=wait_until(lambda: next(iter(windows()-before),None))
            wait_until(lambda: fullscreen(window)==expected)
            # Loading is asynchronous. Wait for the main menu's original art.
            def menu_ready():
                xx,yy,ww,hh=geometry(window)
                im=ImageGrab.grab(bbox=(xx,yy,xx+ww,yy+hh),xdisplay=args.display)
                return sum(g>50 and g>r*1.4 and g>b*1.2 for r,g,b in im.convert('RGB').resize((160,120)).getdata())>100
            wait_until(menu_ready)
            x.XSetInputFocus(d,window,2,0); x.XFlush(d)
            if not expected:
                assert geometry(window)[2:]==(960,720), geometry(window)
                x.XMoveResizeWindow(d,window,0,0,1100,600); x.XFlush(d)
                wait_until(lambda: geometry(window)[2:]==(1100,600))
                time.sleep(.4)
                im=ImageGrab.grab(bbox=(0,0,1100,600),xdisplay=args.display)
                im.save(args.output/(name+'-resized.png'))
                # 640:480 content occupies 800 pixels, leaving 150 on each side.
                assert im.getpixel((25,300))[:3]==(0,0,0), 'left letterbox is missing'
                assert im.getpixel((1075,300))[:3]==(0,0,0), 'right letterbox is missing'
            key('F11')
            wait_until(lambda: fullscreen(window)!=expected)
            assert json.loads(settings.read_text())['fullscreen']!=expected
            key('F11')
            wait_until(lambda: fullscreen(window)==expected)
            assert json.loads(settings.read_text())['fullscreen']==expected
            key('Escape')
            process.wait(timeout=10)
            assert process.returncode==0
            checks.append({'case':name,'passed':True,'fullscreen':expected,'flags':flags,'saved_before':saved,'resized':not expected,'f11_both_directions':True})
        finally:
            if process.poll() is None:
                process.terminate(); process.wait(timeout=5)
summary={'passed':True,'display':args.display,'checks':checks,'limitations':['Fullscreen EWMH property and settings changes were verified on Xvfb without a window manager; monitor-sized fullscreen presentation requires a desktop session.','No physical controller was exercised.']}
(args.output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
