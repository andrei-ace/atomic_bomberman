#!/usr/bin/env python3
"""Observe automatic game startup on the real GNOME desktop; no synthetic input.

Set DISPLAY to the intended desktop. Uses explicit Mesa software rendering
for hosts where this process cannot access GPU devices. Runs three short games.
"""
import os,time,subprocess,json,ctypes as C
from pathlib import Path
root=Path(__file__).resolve().parents[1]
probe_display=os.environ.get('DISPLAY')
if not probe_display:raise SystemExit('Set DISPLAY to the desktop to check (for example DISPLAY=:1).')
x=C.CDLL('libX11.so.6')
P=C.c_void_p;W=C.c_ulong
x.XOpenDisplay.argtypes=[C.c_char_p];x.XOpenDisplay.restype=P
x.XDefaultRootWindow.argtypes=[P];x.XDefaultRootWindow.restype=W
x.XQueryTree.argtypes=[P,W,C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(W)),C.POINTER(C.c_uint)]
x.XFetchName.argtypes=[P,W,C.POINTER(C.c_char_p)]
x.XFree.argtypes=[P]
x.XInternAtom.argtypes=[P,C.c_char_p,C.c_int];x.XInternAtom.restype=W
x.XGetWindowProperty.argtypes=[P,W,W,C.c_long,C.c_long,C.c_int,W,C.POINTER(W),C.POINTER(C.c_int),C.POINTER(W),C.POINTER(W),C.POINTER(P)]
x.XGetGeometry.argtypes=[P,W,C.POINTER(W),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint)]
error_handler=C.CFUNCTYPE(C.c_int,P,P)(lambda d,e:0)
x.XSetErrorHandler.argtypes=[C.c_void_p]
x.XSetErrorHandler(C.cast(error_handler,C.c_void_p))
d=x.XOpenDisplay(os.fsencode(probe_display));assert d,'Host display inaccessible'
root_window=x.XDefaultRootWindow(d)
full_atom=x.XInternAtom(d,b'_NET_WM_STATE_FULLSCREEN',0)
def children(w):
    rr,par,ptr,count=W(),W(),C.POINTER(W)(),C.c_uint()
    if not x.XQueryTree(d,w,C.byref(rr),C.byref(par),C.byref(ptr),C.byref(count)):return []
    result=[ptr[i]for i in range(count.value)]
    if ptr:x.XFree(ptr)
    return result

def prop(w,key):
    atom=x.XInternAtom(d,key.encode(),0)
    at,fmt,n,left,data=W(),C.c_int(),W(),W(),P()
    x.XGetWindowProperty(d,w,atom,0,1024,0,0,C.byref(at),C.byref(fmt),C.byref(n),C.byref(left),C.byref(data))
    if data.value and fmt.value==8:value=C.string_at(data,n.value).decode(errors='replace')
    elif data.value and fmt.value==32:value=list((W*n.value).from_address(data.value))
    else:value=None
    if data.value:x.XFree(data)
    return value

def own_window():
    candidates=prop(root_window,'_NET_CLIENT_LIST') or children(root_window)
    candidates=[w for w in candidates if w not in baseline]
    for w in candidates:
        name=prop(w,'_NET_WM_NAME')
        if isinstance(name,str)and name.startswith('Atomic Bomberman'):
            return w
    return None

def window_state(w):
    atom=x.XInternAtom(d,b'_NET_WM_STATE',0)
    at,fmt,n,left,data=W(),C.c_int(),W(),W(),P()
    x.XGetWindowProperty(d,w,atom,0,64,0,0,C.byref(at),C.byref(fmt),C.byref(n),C.byref(left),C.byref(data))
    values=list((W*n.value).from_address(data.value))if data.value and fmt.value==32 else []
    if data.value:x.XFree(data)
    rr,xx,yy,ww,hh,border,depth=W(),C.c_int(),C.c_int(),C.c_uint(),C.c_uint(),C.c_uint(),C.c_uint()
    if not x.XGetGeometry(d,w,C.byref(rr),C.byref(xx),C.byref(yy),C.byref(ww),C.byref(hh),C.byref(border),C.byref(depth)):return None
    return dict(fullscreen_property=full_atom in values,size=[ww.value,hh.value])

env=os.environ.copy()
for key in ['ATOMIC_BOMBERMAN_SILENT_AUDIO','ATOMIC_BOMBERMAN_AUDIO_PACE']:env.pop(key,None)
env.update(DISPLAY=probe_display,LIBGL_ALWAYS_SOFTWARE='1',__GLX_VENDOR_LIBRARY_NAME='mesa',LP_NUM_THREADS='2')
settings='/tmp/atomic-real-desktop-settings.json'
Path(settings).unlink(missing_ok=True)
checks=[]
for mode,frames in [('--windowed',360),('--fullscreen',180),('--windowed',180)]:
    baseline=set(prop(root_window,'_NET_CLIENT_LIST')or children(root_window))
    log_path=Path('/tmp/atomic-real-desktop-'+str(len(checks))+'.log')
    with log_path.open('w') as log:
        process=subprocess.Popen([str(root/'target/release/bomberman'),'--assets',str(root/'local-assets'),'--settings',settings,'--seed','1997',mode,'--smoke-test',str(frames)],env=env,stdout=log,stderr=subprocess.STDOUT)
        observed=None
        try:
            deadline=time.monotonic()+45
            while process.poll() is None and time.monotonic()<deadline:
                w=own_window()
                if w:
                    state=window_state(w)
                    if state and state['fullscreen_property']==(mode=='--fullscreen'):observed=state
                time.sleep(.1)
            assert process.wait(timeout=5)==0,log_path.read_text()
        finally:
            if process.poll() is None:process.terminate();process.wait(timeout=5)
    assert observed,(mode,'No matching WM state')
    assert 'playing silently' not in log_path.read_text(),log_path.read_text()
    checks.append(dict(mode=mode,frames=frames,**observed,audio_unmuted=True))
report=dict(passed=True,display=probe_display,window_manager='GNOME Shell',native_desktop=True,software_rendering=True,checks=checks,user_audibility_confirmation='pending',controller='skipped: no physical controller; user authorized skipping',limitations=['Window mode is observed through the real window manager; physical display appearance and audibility require user observation.'])
(root/'docs/evidence/linux-desktop.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
