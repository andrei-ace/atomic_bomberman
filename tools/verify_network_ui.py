#!/usr/bin/env python3
"""Exercise two actual game windows and private invitations on a dedicated X11 display."""
import argparse
import ctypes as C
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from verify_relay import LatencyProxy
import base64


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--display', required=True)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--xclip', type=Path, required=True)
    parser.add_argument('--game', type=Path, default=Path('target/debug/bomberman'))
    parser.add_argument('--relay', type=Path, default=Path('target/release/bomberman-relay'))
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--latency-ms', type=float, default=0)
    args = parser.parse_args()
    x = C.CDLL('libX11.so.6'); xt = C.CDLL('libXtst.so.6')
    pointer, window_type = C.c_void_p, C.c_ulong
    x.XOpenDisplay.argtypes = [C.c_char_p]; x.XOpenDisplay.restype = pointer
    x.XDefaultRootWindow.argtypes = [pointer]; x.XDefaultRootWindow.restype = window_type
    x.XQueryTree.argtypes = [pointer, window_type, C.POINTER(window_type), C.POINTER(window_type), C.POINTER(C.POINTER(window_type)), C.POINTER(C.c_uint)]
    x.XFetchName.argtypes = [pointer, window_type, C.POINTER(C.c_char_p)]
    x.XGetGeometry.argtypes = [pointer,window_type,C.POINTER(window_type),C.POINTER(C.c_int),C.POINTER(C.c_int),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint),C.POINTER(C.c_uint)]
    x.XFree.argtypes = [pointer]
    x.XSetInputFocus.argtypes = [pointer,window_type,C.c_int,C.c_ulong]
    x.XRaiseWindow.argtypes = [pointer,window_type]
    x.XFlush.argtypes = [pointer]
    x.XKeysymToKeycode.argtypes = [pointer,C.c_ulong]; x.XKeysymToKeycode.restype = C.c_uint
    x.XStringToKeysym.argtypes = [C.c_char_p]; x.XStringToKeysym.restype = C.c_ulong
    xt.XTestFakeKeyEvent.argtypes = [pointer,C.c_uint,C.c_int,C.c_ulong]
    display = x.XOpenDisplay(args.display.encode()); assert display
    root_window = x.XDefaultRootWindow(display)
    environment = dict(os.environ, DISPLAY=args.display, LIBGL_ALWAYS_SOFTWARE='1', LP_NUM_THREADS='2')

    def windows():
        root, parent, children, count = window_type(),window_type(),C.POINTER(window_type)(),C.c_uint()
        x.XQueryTree(display,root_window,C.byref(root),C.byref(parent),C.byref(children),C.byref(count))
        result = []
        for i in range(count.value):
            rr,xx,yy,ww,hh,border,depth=window_type(),C.c_int(),C.c_int(),C.c_uint(),C.c_uint(),C.c_uint(),C.c_uint()
            x.XGetGeometry(display,children[i],C.byref(rr),C.byref(xx),C.byref(yy),C.byref(ww),C.byref(hh),C.byref(border),C.byref(depth))
            if ww.value >= 640 and hh.value >= 480: result.append(children[i])
            name = C.c_char_p()
            if x.XFetchName(display,children[i],C.byref(name)) and name.value:
                if name.value.startswith(b'Atomic Bomberman'):
                    result.append(children[i])
                x.XFree(name)
        if children: x.XFree(children)
        return set(result)

    probes = {}
    active_probe = [None]
    def focus(window):
        active_probe[0] = probes.get(window)
        x.XRaiseWindow(display,window); x.XSetInputFocus(display,window,2,0); x.XFlush(display); time.sleep(.1)

    def key(name, down=True):
        code = x.XKeysymToKeycode(display,x.XStringToKeysym(name.encode())); assert code
        xt.XTestFakeKeyEvent(display,code,int(down),0); x.XFlush(display)

    def press(name):
        key(name); time.sleep(.06); key(name,False); time.sleep(.12)

    def select(index):
        probe = active_probe[0]
        probe.unlink(missing_ok=True); press('F12')
        deadline=time.monotonic()+30
        while not probe.exists() and time.monotonic()<deadline:
            time.sleep(.2); press('F12')
        state = json.loads(probe.read_text())
        count = {'Main':9,'Online':4,'NetworkLobby':5,'NetworkDevices':13,'Battle':4}[state['screen']]
        for _ in range((index-state['selection'])%count): press('Down')
        press('Return')

    clipboard_jobs = []
    def paste(text):
        process = subprocess.Popen([str(args.xclip.resolve()),'-selection','clipboard','-in','-quiet','-loops','1'],
                                   stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=environment)
        process.stdin.write(text.encode()); process.stdin.close(); clipboard_jobs.append(process); time.sleep(.15)
        key('Control_L'); press('v'); key('Control_L',False); time.sleep(.2)

    def clipboard():
        # Miniquad's clipboard owner must continue receiving render events.
        return subprocess.check_output([str(args.xclip.resolve()),'-selection','clipboard','-out'],env=environment,text=True,timeout=5).strip()

    with tempfile.TemporaryDirectory(prefix='atomic online ui ') as temporary:
        base = Path(temporary); processes = []; logs = []
        with socket.socket() as reserve:
            reserve.bind(('127.0.0.1',0)); port=reserve.getsockname()[1]
        state = base/'relay-state'
        relay = str(args.relay.resolve())
        subprocess.run([relay,'init','--state',str(state),'--advertise',f'127.0.0.1:{port}','--listen',f'127.0.0.1:{port}'],check=True,capture_output=True)
        invitation = subprocess.check_output([relay,'export-invite','--state',str(state)],text=True).strip()
        proxy = LatencyProxy(port, args.latency_ms) if args.latency_ms else None
        if proxy:
            value = invitation[4:]
            decoded = json.loads(base64.urlsafe_b64decode(value+'='*((-len(value))%4)))
            decoded['server']['endpoint'] = f'wss://127.0.0.1:{proxy.port}/v1/ws'
            invitation = 'AB1.'+base64.urlsafe_b64encode(json.dumps(decoded).encode()).decode().rstrip('=')
        log=(base/'relay.log').open('w'); logs.append(log)
        processes.append(subprocess.Popen([relay,'serve','--state',str(state)],stdout=log,stderr=log))
        time.sleep(.3)

        def launch(name):
            before=windows(); directory=base/name; directory.mkdir(); probe=directory/'probe.json'
            env=dict(environment,XDG_CONFIG_HOME=str(directory/'config'),ATOMIC_BOMBERMAN_STATE_PROBE=str(probe))
            log=(directory/'game.log').open('w'); logs.append(log)
            process=subprocess.Popen([str(args.game.resolve()),'--assets',str(args.assets.resolve()),'--settings',str(directory/'settings.json'),'--mute'],env=env,stdout=log,stderr=log)
            processes.append(process)
            deadline=time.monotonic()+120
            while time.monotonic()<deadline:
                candidates=windows()-before
                if candidates:
                    window=candidates.pop(); probes[window]=probe; focus(window); time.sleep(2); return window,directory,probe
                assert process.poll() is None
                time.sleep(.1)
            raise AssertionError('game window unavailable: '+(directory/'game.log').read_text()[-2000:])

        def snapshot(player, screen=None):
            window,directory,probe=player
            focus(window); probe.unlink(missing_ok=True); press('F12')
            deadline=time.monotonic()+120
            while not probe.exists() and time.monotonic()<deadline:
                time.sleep(.3); press('F12')
            assert probe.exists(), 'probe unavailable: '+(directory/'game.log').read_text()[-2000:]
            value=json.loads(probe.read_text())
            if screen: assert value['screen']==screen, (screen,value['screen'],value.get('message'))
            return value

        try:
            host=launch('host'); snapshot(host,'Main')
            select(5); snapshot(host,'Online'); paste(invitation); select(1); time.sleep(.5)
            state_host=snapshot(host,'NetworkLobby'); assert state_host['network']['id']==1
            profile=host[1]/'config/atomic-bomberman/connections.json'; assert profile.exists()
            assert profile.stat().st_mode&0o777==0o600
            select(1); game_invitation=clipboard(); assert game_invitation.startswith('AB1.')
            friend=launch('friend'); snapshot(friend,'Main'); select(6); paste(game_invitation); select(1); time.sleep(.5)
            state_friend=snapshot(friend,'NetworkLobby'); assert state_friend['network']['id']==2
            # A second keyboard is assigned locally through the actual lobby UI.
            select(3); snapshot(friend,'NetworkDevices'); select(1); press('Escape'); time.sleep(.3)
            focus(host[0]); state_host=snapshot(host,'NetworkLobby')
            assert state_host['network']['lobby']['owners'].count(2)==2
            focus(friend[0]); press('Return')
            focus(host[0]); select(0); time.sleep(.3); select(2); time.sleep(2)
            snapshot(friend,'Battle'); snapshot(host,'Battle')
            press('p'); time.sleep(.4)
            paused_host=snapshot(host,'Battle'); paused_friend=snapshot(friend,'Battle')
            assert paused_host['paused'] and paused_friend['paused']
            # Once paused, the authoritative game state must be byte-for-byte equal.
            assert paused_host['simulation']==paused_friend['simulation']
            # Host returns to lobby after unpausing and ending/restarting through UI.
            focus(host[0]); press('Return'); time.sleep(.2)
            focus(friend[0]); press('Escape'); time.sleep(.3)
            # Escape requests shared pause. The host's leave option closes the room.
            focus(host[0]); select(3); time.sleep(.6)
            assert snapshot(friend)['network'] is None
            assert snapshot(host)['network'] is None
            # Returning host uses saved profile; friend reuses its new short code.
            focus(host[0]); select(5); select(1); time.sleep(.4)
            state_host=snapshot(host,'NetworkLobby'); code=state_host['network']['room']
            focus(friend[0]); select(6); paste(code); select(1); time.sleep(.4)
            assert snapshot(friend,'NetworkLobby')['network']['id']==2
            for log in logs: log.flush()
            for file in base.rglob('*.log'):
                text=file.read_text()
                assert invitation not in text and game_invitation not in text
            report={'passed':True,'platform':'Ubuntu 24.04 x86-64, isolated Xvfb, Mesa software rendering',
                    'injected_round_trip_tcp_delay_ms':args.latency_ms,
                    'checks':['fresh server invitation pasted with Ctrl-V','create room','private profile mode 0600',
                              'Copy Invite through system clipboard','fresh friend joins with one full invitation',
                              'multiple local keyboards/controllers in lobby','readiness and start','actual two-window battle',
                              'shared pause and identical serialized simulation','host departure ends session',
                              'saved server reuse','short room-code reuse','credentials absent from logs'],
                    'limitations':['No physical controller tested.','macOS Cmd-V and Linux/macOS WAN play require Mac hardware.']}
            args.report.parent.mkdir(parents=True,exist_ok=True); args.report.write_text(json.dumps(report,indent=2)+'\n')
            print(json.dumps(report,indent=2))
        finally:
            for process in clipboard_jobs+processes:
                if process.poll() is None: process.terminate()
            for process in clipboard_jobs+processes:
                try: process.wait(timeout=5)
                except subprocess.TimeoutExpired: process.kill(); process.wait()
            for log in logs: log.close()
            if proxy: proxy.close()


if __name__=='__main__': main()
