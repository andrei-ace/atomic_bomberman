#!/usr/bin/env python3
"""Exercise two humans on one keyboard and one human/nine bots in native X11.

Uses the debug binary's opt-in F12 state probe to verify simulation changes
caused by XTest events, rather than inferring movement from rendered pixels.
Requires Pillow, libX11/libXtst and a dedicated X11/Xvfb display.
"""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import time
from PIL import ImageGrab

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--exe',type=Path,default=Path('target/debug/bomberman'))
parser.add_argument('--assets',type=Path,default=Path('local-assets'))
parser.add_argument('--display',default=os.environ.get('DISPLAY',':99'))
parser.add_argument('--output',type=Path,default=Path('/tmp/atomic-bomberman-keyboard'))
args=parser.parse_args()
args.output.mkdir(parents=True,exist_ok=True)
x,xt=C.CDLL('libX11.so.6'),C.CDLL('libXtst.so.6')
W,P=C.c_ulong,C.c_void_p
x.XOpenDisplay.argtypes,x.XOpenDisplay.restype=[C.c_char_p],P
x.XDefaultRootWindow.argtypes,x.XDefaultRootWindow.restype=[P],W
x.XQueryTree.argtypes=[P,W,C.POINTER(W),C.POINTER(W),C.POINTER(C.POINTER(W)),C.POINTER(C.c_uint)]
x.XFree.argtypes=[P]
x.XSetInputFocus.argtypes=[P,W,C.c_int,W]
x.XFlush.argtypes=[P]
x.XStringToKeysym.argtypes,x.XStringToKeysym.restype=[C.c_char_p],W
x.XKeysymToKeycode.argtypes,x.XKeysymToKeycode.restype=[P,W],C.c_uint
xt.XTestFakeKeyEvent.argtypes=[P,C.c_uint,C.c_int,W]
d=x.XOpenDisplay(os.fsencode(args.display));assert d,'owned X11 display is unavailable'
root=x.XDefaultRootWindow(d)
probe=args.output/'state.json'
held=set()

def windows():
    rr,parent,count,children=W(),W(),C.c_uint(),C.POINTER(W)()
    x.XQueryTree(d,root,C.byref(rr),C.byref(parent),C.byref(children),C.byref(count))
    result={children[i] for i in range(count.value)}
    if children:x.XFree(children)
    return result

def keys(names,down):
    for name in names:
        code=x.XKeysymToKeycode(d,x.XStringToKeysym(name.encode()))
        assert code,name
        xt.XTestFakeKeyEvent(d,code,int(down),0)
        if down:held.add(name)
        else:held.discard(name)
    x.XFlush(d)

def key(name):
    keys([name],True);time.sleep(.06);keys([name],False);time.sleep(.07)

def wait_for(predicate,timeout=90):
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        value=predicate()
        if value:return value
        assert process.poll() is None,'game exited during keyboard verification'
        time.sleep(.03)
    raise AssertionError('timed out waiting for native input/state')

def capture(name):
    probe.unlink(missing_ok=True)
    key('F12')
    wait_for(lambda:probe.exists(),5)
    state=json.loads(probe.read_text())
    (args.output/(name+'.json')).write_text(json.dumps(state,indent=2)+'\n')
    assert state['screen']=='Battle',state['screen']
    assert not state['paused'],'game paused during player input'
    return state['simulation']

def playing(prefix):
    key('Return')
    deadline=time.monotonic()+15
    while time.monotonic()<deadline:
        state=capture(prefix+'-countdown')
        if state['phase']=='Playing':return state
        time.sleep(.15)
    raise AssertionError('battle did not leave the countdown')

def positions(state):return [p['position'] for p in state['players']]

def screenshot(name):
    ImageGrab.grab(bbox=(0,0,960,720),xdisplay=args.display).save(args.output/(name+'.png'))

manifest=json.loads((args.assets/'manifest.json').read_text())
basic=next(i for i,s in enumerate(manifest['schemes']) if s['id'].upper().endswith('/BASIC.SCH'))
checks=[]
for case,devices in [('two-humans',['Keyboard1','Keyboard2']),('one-human-nine-bots',['Keyboard1']+['Bot']*9)]:
    settings=args.output/(case+'-settings.json')
    settings.write_text(json.dumps({'participant_count':len(devices),'participants':[{'device':device,'team':i//5}for i,device in enumerate(devices)],'scheme':basic,'random_starts':False,'sudden_death':False,'round_seconds':150,'gold':False,'fullscreen':False}))
    before=windows()
    with open(args.output/(case+'.log'),'w') as log:
        process=subprocess.Popen([str(args.exe.resolve()),'--assets',str(args.assets.resolve()),'--settings',str(settings),'--windowed','--seed','1997','--mute'],stdout=log,stderr=log,env={**os.environ,'DISPLAY':args.display,'LP_NUM_THREADS':'2','ATOMIC_BOMBERMAN_STATE_PROBE':str(probe.resolve())})
        try:
            window=wait_for(lambda:next(iter(windows()-before),None))
            def menu_ready():
                im=ImageGrab.grab(bbox=(0,0,960,720),xdisplay=args.display).convert('RGB').resize((160,120))
                return sum(g>50 and g>r*1.4 and g>b*1.2 for r,g,b in im.getdata())>100
            wait_for(menu_ready)
            x.XSetInputFocus(d,window,2,0);x.XFlush(d)
            baseline=playing(case)
            assert len(baseline['players'])==len(devices)
            if case=='two-humans':
                start=positions(baseline)
                keys(['Right'],True);time.sleep(.18)
                first=capture('keyboard1-alone');keys(['Right'],False)
                assert positions(first)[0]['x']>start[0]['x']
                assert positions(first)[1]==start[1]
                released1=capture('keyboard1-released')
                assert all(i['direction'] is None for i in released1['observed_inputs'])
                keys(['a'],True);time.sleep(.18)
                second=capture('keyboard2-alone');keys(['a'],False)
                assert positions(second)[1]['x']<positions(released1)[1]['x']
                assert positions(second)[0]==positions(released1)[0]
                released2=capture('keyboard2-released')
                assert all(i['direction'] is None for i in released2['observed_inputs'])
                keys(['Left','d'],True);time.sleep(.18)
                both=capture('both-keyboards-moving');keys(['Left','d'],False)
                assert positions(both)[0]['x']<positions(released2)[0]['x']
                assert positions(both)[1]['x']>positions(released2)[1]['x']
                assert [i['direction'] for i in both['observed_inputs']]==['Left','Right']
                keys(['space'],True);time.sleep(.05)
                bomb1=capture('keyboard1-bomb');keys(['space'],False)
                assert any(b['owner']==0 for b in bomb1['bombs'])
                assert not any(b['owner']==1 for b in bomb1['bombs'])
                keys(['Shift_L'],True);time.sleep(.05)
                bomb2=capture('keyboard2-bomb');keys(['Shift_L'],False)
                assert {b['owner']for b in bomb2['bombs']}=={0,1}
                screenshot('two-humans-bombs')
                # Start a fresh match through the actual pause menu so both
                # players again have capacity for simultaneous bomb input.
                key('Escape');key('Down');key('Down');key('Return')
                deadline=time.monotonic()+15
                while True:
                    state=capture('restart-countdown')
                    if state['phase']=='Playing':break
                    assert time.monotonic()<deadline
                    time.sleep(.15)
                keys(['space','Shift_L'],True);time.sleep(.05)
                simultaneous=capture('both-keyboards-bombs');keys(['space','Shift_L'],False)
                assert {b['owner']for b in simultaneous['bombs']}=={0,1}
                assert all(i['bomb'] for i in simultaneous['observed_inputs'])
                assert all(p['alive']for p in simultaneous['players'])
                screenshot('two-humans-simultaneous')
                checks.append({'case':case,'passed':True,'movement':['arrows separately','WASD separately','arrows and WASD simultaneously'],'bombs':['Space separately','left Shift separately','Space and left Shift simultaneously'],'positions_before':start,'positions_after':positions(both),'simultaneous_bomb_owners':sorted({b['owner']for b in simultaneous['bombs']}),'unexpected_pause_or_menu':False})
            else:
                before_positions=positions(baseline)
                time.sleep(.35)
                active=capture('one-human-nine-bots-active')
                assert [p['config']['control']for p in active['players']]==['Human']+['Bot']*9
                active_bots=[p['id']for p in active['players'][1:]if p['position']!=before_positions[p['id']]]
                assert active_bots or any(b['owner']!=0 for b in active['bombs']),'bots did not act'
                screenshot('one-human-nine-bots')
                checks.append({'case':case,'passed':True,'participants':10,'humans':1,'bots':9,'bots_observed_moving':active_bots,'bomb_owners':sorted({b['owner']for b in active['bombs']})})
            key('Escape');key('Down');key('Down');key('Down');key('Return');key('Escape')
            process.wait(timeout=10);assert process.returncode==0
        finally:
            keys(list(held),False)
            if process.poll() is None:process.terminate();process.wait(timeout=5)
summary={'passed':True,'display':args.display,'input_source':'native X11 XTest events','observation':'debug-build F12 state snapshot after the real Macroquad input path and Simulation.step','checks':checks,'limitations':['Virtual X11 key input verifies software keyboard handling; physical keyboard rollover limits vary by hardware.','No physical controller was exercised.']}
(args.output/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
