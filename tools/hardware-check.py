#!/usr/bin/env python3
"""Inventory Linux physical test prerequisites; optionally run the live game-path check.

No synthetic input or inferred speaker pass is used. --interactive opens a
separate window with explicit controller, stereo and fullscreen instructions.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import time


def x11_inventory(display):
    result={'display':display or None,'available':False,'window_manager':None}
    if not display:return result
    try:
        x=C.CDLL('libX11.so.6');P,W=C.c_void_p,C.c_ulong
        x.XOpenDisplay.argtypes,x.XOpenDisplay.restype=[C.c_char_p],P
        x.XCloseDisplay.argtypes=[P]
        x.XDefaultRootWindow.argtypes,x.XDefaultRootWindow.restype=[P],W
        x.XInternAtom.argtypes,x.XInternAtom.restype=[P,C.c_char_p,C.c_int],W
        x.XGetWindowProperty.argtypes=[P,W,W,C.c_long,C.c_long,C.c_int,W,C.POINTER(W),C.POINTER(C.c_int),C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(C.c_ubyte))]
        x.XFree.argtypes=[P]
        d=x.XOpenDisplay(os.fsencode(display))
        if not d:return result
        result['available']=True
        try:
            def prop(window,name):
                atom=x.XInternAtom(d,name.encode(),1)
                if not atom:return None
                kind,fmt,count,remaining,data=W(),C.c_int(),W(),W(),C.POINTER(C.c_ubyte)()
                status=x.XGetWindowProperty(d,window,atom,0,1024,0,0,C.byref(kind),C.byref(fmt),C.byref(count),C.byref(remaining),C.byref(data))
                if status!=0 or not data:return None
                try:
                    if fmt.value==32:return [C.cast(data,C.POINTER(W))[i] for i in range(count.value)]
                    if fmt.value==8:return bytes(data[:count.value]).decode('utf8','replace')
                    return None
                finally:x.XFree(data)
            manager=prop(x.XDefaultRootWindow(d),'_NET_SUPPORTING_WM_CHECK')
            if manager:result['window_manager']=prop(manager[0],'_NET_WM_NAME')
        finally:x.XCloseDisplay(d)
    except (OSError,ValueError)as error:result['error']=str(error)
    return result


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--exe',type=Path,default=Path('target/debug/examples/hardware_check'))
    p.add_argument('--display',default=os.environ.get('DISPLAY',''))
    p.add_argument('--interactive',action='store_true')
    p.add_argument('--report',type=Path,default=Path('docs/evidence/linux-hardware.json'))
    p.add_argument('--require-complete',action='store_true')
    a=p.parse_args()
    a.report.parent.mkdir(parents=True,exist_ok=True)
    inputs=[str(x)for x in Path('/dev/input').glob('event*')]
    sounds=[str(x)for x in Path('/dev/snd').glob('pcm*')]
    runtime=Path(os.environ.get('XDG_RUNTIME_DIR',f'/run/user/{os.getuid()}'))
    audio_sockets=[str(x)for x in [runtime/'pulse/native',runtime/'pipewire-0']if x.exists()]
    inventory={'status':'not_run','devices':[]}
    if a.exe.exists():
        backend=a.report.with_name(a.report.stem+'-controllers.json')
        r=subprocess.run([str(a.exe.resolve()),'--inventory',str(backend.resolve())],capture_output=True,text=True)
        if r.returncode==0:inventory=json.loads(backend.read_text())
        else:inventory={'status':'error','devices':[],'error':r.stderr.strip()}
    else:inventory['instructions']='Build with: cargo build -p bomberman --example hardware_check'
    display=x11_inventory(a.display)
    report={'timestamp_unix':int(time.time()),'status':'incomplete','physical_prerequisites':{'input_event_nodes':inputs,'audio_pcm_nodes':sounds,'desktop_audio_sockets':audio_sockets,'controller_backend':inventory,'x11':display,'wayland_display':os.environ.get('WAYLAND_DISPLAY')},'checks':{'controller':{'status':'available_not_exercised'if inventory.get('devices')else'unavailable','passed':False},'speakers':{'status':'available_not_listened'if sounds or audio_sockets else'unavailable','passed':False},'desktop_fullscreen':{'status':'available_not_exercised'if display.get('window_manager')else'unavailable','passed':False}},'interactive':None,'instructions':'On the Linux desktop with a connected gamepad and speakers: python3 tools/hardware-check.py --interactive --report /tmp/atomic-hardware.json','limitations':['Device discovery does not establish that physical input/output works.','A virtual X11 display, ALSA file/null sink, or synthetic event is not accepted as physical verification.']}
    if a.interactive:
        if not display['available']:report['interactive']={'status':'unavailable','reason':'No accessible X11 desktop display; run from the target desktop session.'}
        elif not a.exe.exists():report['interactive']={'status':'unavailable','reason':'Hardware check executable has not been built.'}
        else:
            live=a.report.with_name(a.report.stem+'-interactive.json')
            r=subprocess.run([str(a.exe.resolve()),'--interactive',str(live.resolve())],env={**os.environ,'DISPLAY':a.display})
            if r.returncode==0 and live.exists():
                report['interactive']=json.loads(live.read_text())
                for key,source in [('controller','controller'),('speakers','audio'),('desktop_fullscreen','display')]:
                    passed=bool(report['interactive'][source]['passed'])
                    if key=='desktop_fullscreen'and not display.get('window_manager'):passed=False
                    report['checks'][key]={'status':'passed'if passed else'incomplete','passed':passed}
            else:report['interactive']={'status':'error','returncode':r.returncode}
    if all(v['passed']for v in report['checks'].values()):report['status']='passed'
    elif all(v['status']=='unavailable'for v in report['checks'].values()):report['status']='unavailable'
    a.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
    if a.require_complete and report['status']!='passed':raise SystemExit(1)

if __name__=='__main__':main()
