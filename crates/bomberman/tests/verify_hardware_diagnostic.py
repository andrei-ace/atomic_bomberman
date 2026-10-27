#!/usr/bin/env python3
"""Verify the physical diagnostic never passes from inventory or stray confirmations."""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import time

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--exe',type=Path,default=Path('target/debug/examples/hardware_check'))
p.add_argument('--display',default=os.environ.get('DISPLAY',':99'))
p.add_argument('--output',type=Path,default=Path('/tmp/atomic-hardware-diagnostic'))
a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
x,xt=C.CDLL('libX11.so.6'),C.CDLL('libXtst.so.6');P,W=C.c_void_p,C.c_ulong
x.XOpenDisplay.argtypes,x.XOpenDisplay.restype=[C.c_char_p],P
x.XDefaultRootWindow.argtypes,x.XDefaultRootWindow.restype=[P],W
x.XQueryTree.argtypes=[P,W,C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(W)),C.POINTER(C.c_uint)]
x.XFree.argtypes=[P];x.XSetInputFocus.argtypes=[P,W,C.c_int,W];x.XFlush.argtypes=[P]
x.XStringToKeysym.argtypes,x.XStringToKeysym.restype=[C.c_char_p],W
x.XKeysymToKeycode.argtypes,x.XKeysymToKeycode.restype=[P,W],C.c_uint
xt.XTestFakeKeyEvent.argtypes=[P,C.c_uint,C.c_int,W]
d=x.XOpenDisplay(os.fsencode(a.display));assert d,'private display unavailable'
root=x.XDefaultRootWindow(d)
def windows():
    rr,parent,count,children=W(),W(),C.c_uint(),C.POINTER(W)()
    x.XQueryTree(d,root,C.byref(rr),C.byref(parent),C.byref(children),C.byref(count))
    found={children[i]for i in range(count.value)}
    if children:x.XFree(children)
    return found

def key(name):
    code=x.XKeysymToKeycode(d,x.XStringToKeysym(name.encode()))
    xt.XTestFakeKeyEvent(d,code,1,0);x.XFlush(d);time.sleep(.12)
    xt.XTestFakeKeyEvent(d,code,0,0);x.XFlush(d);time.sleep(.2)

inventory=a.output/'inventory.json'
subprocess.run([str(a.exe.resolve()),'--inventory',str(inventory.resolve())],check=True)
assert 'devices'in json.loads(inventory.read_text())
before=windows();live=a.output/'interactive.json'
with open(a.output/'run.log','w')as log:
    process=subprocess.Popen([str(a.exe.resolve()),'--interactive',str(live.resolve())],stdout=log,stderr=log,env={**os.environ,'DISPLAY':a.display,'LP_NUM_THREADS':'2','ATOMIC_BOMBERMAN_SILENT_AUDIO':'1'})
    try:
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            found=windows()-before
            if found:break
            assert process.poll()is None,'diagnostic exited early'
            time.sleep(.05)
        assert found,'diagnostic did not create a window'
        x.XSetInputFocus(d,next(iter(found)),2,0);x.XFlush(d);time.sleep(.8)
        # Confirming without a played speaker test or a fullscreen transition
        # must leave all hardware gates incomplete.
        key('y');key('v');key('Escape')
        process.wait(timeout=10);assert process.returncode==0
    finally:
        if process.poll()is None:process.terminate();process.wait(timeout=5)
report=json.loads(live.read_text())
assert report['status']=='incomplete'
assert not report['audio']['passed']
assert report['audio']['attempts']==[0,0]
assert report['audio']['heard_correct_channel_user_confirmed']==[False,False]
assert not report['display']['fullscreen_visually_confirmed_by_user']
assert not report['display']['passed']
assert not report['controller']['passed']
summary={'passed':True,'checks':['Gilrs inventory initializes without claiming exercise','stray speaker confirmation cannot pass an unplayed channel','stray display confirmation cannot pass windowed mode','missing gamepad events cannot pass the controller gate','partial report saved and clean exit'],'physical_hardware_verified':False,'harness':'crates/bomberman/tests/verify_hardware_diagnostic.py'}
(a.output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');print(json.dumps(summary,indent=2))
