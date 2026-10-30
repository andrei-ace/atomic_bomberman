#!/usr/bin/env python3
"""Exercise two actual game windows and private invitations on a dedicated X11 display."""
import argparse
import ctypes as C
import json
import os
import signal
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from verify_relay import parse_invitation
from verify_datagram import ImpairmentProxy
import base64
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--display', default=os.environ.get('DISPLAY'), required=not bool(os.environ.get('DISPLAY')))
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--xclip', type=Path, required=True)
    parser.add_argument('--game', type=Path, default=Path('target/debug/bomberman'))
    parser.add_argument('--relay', type=Path, default=Path('target/release/bomberman-relay'))
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--latency-ms', type=float, default=0)
    parser.add_argument('--jitter-ms', type=float, default=0)
    parser.add_argument('--loss-percent', type=float, default=0)
    parser.add_argument('--spike-ms', type=float, default=0, help='Extra UDP packet delay during recurring spikes')
    parser.add_argument('--spike-duration-ms', type=float, default=100)
    parser.add_argument('--spike-period-ms', type=float, default=3000)
    parser.add_argument('--live-invite-stdin', action='store_true', help='Use the deployed server link on stdin instead of starting a local relay')
    parser.add_argument('--playability', action='store_true', help='Measure OS-key-to-frame timing with opt-in traces')
    parser.add_argument('--single-play', action='store_true', help='Measure the offline local battle with the same key sequence')
    parser.add_argument('--lan-menu', action='store_true', help='Verify role-aware menus through two actual LAN windows')
    parser.add_argument('--expected-version', help='Reject an incorrectly copied game binary before connecting')
    args = parser.parse_args()
    if args.single_play:
        if args.live_invite_stdin:parser.error('Single play does not use a relay invitation')
        args.playability=True
    if args.lan_menu and (args.single_play or args.playability or args.live_invite_stdin or any((args.latency_ms,args.jitter_ms,args.loss_percent,args.spike_ms))):
        parser.error('LAN menu verification uses the direct local LAN transport')
    if min(args.latency_ms,args.jitter_ms,args.loss_percent,args.spike_ms,args.spike_duration_ms) < 0 or args.loss_percent > 100:
        parser.error('Delays must be nonnegative and loss must be between 0 and 100 percent')
    if args.spike_period_ms <= 0 or args.spike_duration_ms > args.spike_period_ms:
        parser.error('Spike period must be positive and at least as long as its duration')
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

    class WindowAttributes(C.Structure):
        _fields_ = [(name, kind) for name,kind in [
            ('x',C.c_int),('y',C.c_int),('width',C.c_int),('height',C.c_int),('border_width',C.c_int),('depth',C.c_int),
            ('visual',C.c_void_p),('root',C.c_ulong),('window_class',C.c_int),('bit_gravity',C.c_int),('win_gravity',C.c_int),
            ('backing_store',C.c_int),('backing_planes',C.c_ulong),('backing_pixel',C.c_ulong),('save_under',C.c_int),
            ('colormap',C.c_ulong),('map_installed',C.c_int),('map_state',C.c_int),('all_event_masks',C.c_long),
            ('your_event_mask',C.c_long),('do_not_propagate_mask',C.c_long),('override_redirect',C.c_int),('screen',C.c_void_p)]]
    x.XGetWindowAttributes.argtypes = [pointer,window_type,C.POINTER(WindowAttributes)]

    probes = {}
    player_processes = {}
    active_probe = [None]
    def focus(window):
        active_probe[0] = probes.get(window)
        deadline=time.monotonic()+10
        attributes=WindowAttributes()
        while time.monotonic()<deadline:
            if x.XGetWindowAttributes(display,window,C.byref(attributes)) and attributes.map_state==2: break
            time.sleep(.05)
        assert attributes.map_state==2, 'game window did not become viewable'
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
        counts = {'Main':7,'Multiplayer':3,'Internet':len(state['browser']['rooms'])+5,'Online':3,'NetworkDevices':13,'NetworkRules':12,'Battle':4,'Lan':len(state['lan']['games'])+4}
        if state['screen'] == 'Battle':
            count = len(state.get('pause_actions', range(4)))
        elif state['screen'] == 'Results':
            count = len(state.get('results_actions', range(3)))
        elif state['screen'] == 'NetworkLobby':
            count = len(state['network']['actions'])
        elif state['screen'] == 'NetworkColors':
            network = state['network']
            count = network['lobby']['owners'].count(network['id'])+1
        elif state['screen'] == 'NetworkTeams':
            count = len(state['network']['lobby']['owners'])+1
        else:
            count = counts[state['screen']]
        for _ in range((index-state['selection'])%count): press('Down')
        press('Return')

    clipboard_jobs = []
    def set_clipboard(text):
        process = subprocess.Popen([str(args.xclip.resolve()),'-selection','clipboard','-in','-quiet','-loops','1'],
                                   stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=environment)
        process.stdin.write(text.encode()); process.stdin.close(); clipboard_jobs.append(process); time.sleep(.15)
    def paste(text):
        set_clipboard(text)
        key('Control_L'); press('v'); key('Control_L',False); time.sleep(.2)

    def clipboard():
        # Miniquad's clipboard owner must continue receiving render events.
        return subprocess.check_output([str(args.xclip.resolve()),'-selection','clipboard','-out'],env=environment,text=True,timeout=5).strip()

    with tempfile.TemporaryDirectory(prefix='atomic online ui ') as temporary:
        base = Path(temporary); processes = []; logs = []
        if args.single_play or args.lan_menu:
            invitation='';proxy=None
        elif args.live_invite_stdin:
            if args.latency_ms or args.jitter_ms or args.loss_percent or args.spike_ms: parser.error('Latency proxy is only available for the disposable local relay')
            invitation=sys.stdin.read().strip()
            parse_invitation(invitation)
            proxy=None
        else:
            with socket.socket() as reserve:
                reserve.bind(('127.0.0.1',0)); port=reserve.getsockname()[1]
            state = base/'relay-state'
            relay = str(args.relay.resolve())
            subprocess.run([relay,'init','--state',str(state),'--advertise',f'127.0.0.1:{port}','--listen',f'127.0.0.1:{port}'],check=True,capture_output=True)
            invitation = subprocess.check_output([relay,'export-invite','--state',str(state)],text=True).strip()
            proxy = ImpairmentProxy(port, args.latency_ms, args.jitter_ms, args.loss_percent,
                spike_ms=args.spike_ms, spike_duration_ms=args.spike_duration_ms if args.spike_ms else 0,
                spike_period_ms=args.spike_period_ms) if any((args.latency_ms,args.jitter_ms,args.loss_percent,args.spike_ms)) else None
            if proxy:
                decoded = parse_invitation(invitation)
                decoded['server']['endpoint'] = f'wss://[::1]:{proxy.port}/v1/ws'
                invitation = 'AB1.'+base64.urlsafe_b64encode(json.dumps(decoded).encode()).decode().rstrip('=')
            log=(base/'relay.log').open('w'); logs.append(log)
            processes.append(subprocess.Popen([relay,'serve','--state',str(state)],stdout=log,stderr=log))
            time.sleep(.3)

        def launch(name):
            before=windows(); directory=base/name; directory.mkdir(); probe=directory/'probe.json'
            env=dict(environment,XDG_CONFIG_HOME=str(directory/'config'),ATOMIC_BOMBERMAN_STATE_PROBE=str(probe))
            if args.playability:
                env.update(ATOMIC_BOMBERMAN_INPUT_TRACE='1',ATOMIC_BOMBERMAN_TEST_CONFIG=str(directory/'test-config.json'))
                (directory/'settings.json').write_text(json.dumps({'participant_count':2,'random_starts':False,'sudden_death':False}))
            log=(directory/'game.log').open('w'); logs.append(log)
            process=subprocess.Popen([str(args.game.resolve()),'--assets',str(args.assets.resolve()),'--settings',str(directory/'settings.json'),'--mute'],env=env,stdout=log,stderr=log)
            processes.append(process)
            deadline=time.monotonic()+120
            while time.monotonic()<deadline:
                candidates=windows()-before
                if candidates:
                    window=candidates.pop(); probes[window]=probe; player_processes[window]=process; focus(window); time.sleep(2); return window,directory,probe
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

        def open_online(player, paste_button=False, link=None, exercise_clear=False):
            link = invitation if link is None else link
            focus(player[0])
            state = snapshot(player)
            if state['screen'] == 'Main': select(1); snapshot(player,'Multiplayer'); select(1)
            elif state['screen'] == 'Multiplayer': select(1)
            state = snapshot(player)
            if state['screen'] == 'Online':
                if paste_button:
                    set_clipboard(link); select(0)
                    assert snapshot(player,'Online')['browser']['text_present'], 'Paste button did not read clipboard'
                else:
                    paste(link)
                    assert snapshot(player,'Online')['browser']['text_present'], 'Ctrl-V did not read clipboard'
                if exercise_clear:
                    for clear_key in ('BackSpace', 'Delete'):
                        press(clear_key)
                        cleared = snapshot(player,'Online')
                        assert not cleared['browser']['text_present'], clear_key+' did not clear the whole private link'
                        assert cleared['browser']['text_bytes']==0
                        select(1)
                        assert not snapshot(player,'Online')['browser']['ready']
                        paste(link)
                    set_clipboard('not a server link'); select(0)
                    invalid = snapshot(player,'Online')
                    assert not invalid['browser']['text_present'], 'Invalid paste retained an old or malformed link'
                    assert 'Invalid server link' in invalid['message']
                    set_clipboard(''); select(0)
                    empty = snapshot(player,'Online')
                    assert not empty['browser']['text_present']
                    assert 'No clipboard text' in empty['message']
                    paste(link)
                    original_bytes = snapshot(player,'Online')['browser']['text_bytes']
                    press('x'); press('space')
                    assert snapshot(player,'Online')['browser']['text_bytes']==original_bytes, 'Typing silently changed a hidden private link'
                select(1)
            deadline = time.monotonic()+30
            while time.monotonic()<deadline:
                state = snapshot(player)
                if state['screen']=='Internet' and state['browser']['ready']: return state
                time.sleep(.2)
            raise AssertionError('Authenticated game browser did not open')

        def join_discovered(player, code):
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                state=snapshot(player,'Internet')
                for i,room in enumerate(state['browser']['rooms']):
                    if room['code']==code:
                        select(i+5); time.sleep(.5); return snapshot(player,'NetworkLobby')
                time.sleep(.3)
            raise AssertionError('Hosted room not discovered')

        def lobby_action(player, action):
            state = snapshot(player, 'NetworkLobby')
            select(state['network']['actions'].index(action))

        def save_playability(report):
            import gzip,hashlib
            raw=report.pop('_raw_traces')
            trace_path=args.report.with_suffix('.traces.json.gz')
            trace_path.parent.mkdir(parents=True,exist_ok=True)
            trace_bytes=gzip.compress(json.dumps(raw,separators=(',',':')).encode(),mtime=0)
            trace_path.write_bytes(trace_bytes)
            report['trace_artifact']=trace_path.name
            report['trace_sha256']=hashlib.sha256(trace_bytes).hexdigest()
            report['game_sha256']=hashlib.sha256(args.game.read_bytes()).hexdigest()
            report['fixture_sha256']=hashlib.sha256(Path(__file__).with_name('measure_playability.py').read_bytes()).hexdigest()
            report['relay_environment']='none' if args.single_play else 'real public TCP/UDP 443' if args.live_invite_stdin else 'disposable relay'
            if not args.live_invite_stdin and not args.single_play:
                report['environment']='Actual game windows; XTest OS key events; Xvfb and Mesa; simulated relay impairment'
                report['impairment']=dict(rtt_ms=args.latency_ms,jitter_ms=args.jitter_ms,loss_percent=args.loss_percent,
                    spike_ms=args.spike_ms,spike_duration_ms=args.spike_duration_ms if args.spike_ms else 0,
                    spike_period_ms=args.spike_period_ms,udp_received=proxy.received if proxy else 0,udp_dropped=proxy.dropped if proxy else 0)
            serialized=json.dumps(report,indent=2)+'\n'
            if invitation:assert invitation not in serialized
            args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(serialized)
            print(json.dumps({'passed':report['passed'],'version':report['version'],'metrics':report['metrics'],'missed_actions':report['missed_actions']},indent=2))
            if not report['passed']:raise SystemExit(1)

        try:
            host=launch('host'); initial=snapshot(host,'Main')
            if args.expected_version:
                assert initial.get('version')==args.expected_version, 'Game binary version does not match the requested baseline/candidate'
            # Historical comparison binaries predate the version footer/probes.
            if tuple(map(int, initial['version'].split('.'))) >= (0, 5, 1):
                assert initial['main_footer'].endswith('v'+initial['version'])
                assert 'Native Rust' not in initial['main_footer']
            if args.lan_menu:
                select(1); snapshot(host,'Multiplayer'); select(0); snapshot(host,'Lan'); select(0)
                assert snapshot(host,'NetworkLobby')['network']['host']
                friend=launch('friend'); snapshot(friend,'Main')
                select(1); snapshot(friend,'Multiplayer'); select(0); snapshot(friend,'Lan')
                select(1); paste('127.0.0.1'); select(0)
                assert not snapshot(friend,'NetworkLobby')['network']['host']
                lobby_action(host,'Start')
                snapshot(host,'Battle'); snapshot(friend,'Battle')
                focus(friend[0]); press('p'); time.sleep(.4)
                paused_host=snapshot(host,'Battle'); paused_friend=snapshot(friend,'Battle')
                assert paused_host['paused'] and paused_friend['paused']
                assert paused_host['pause_actions']==['Resume','Help','Restart','Leave']
                assert paused_friend['pause_actions']==['Resume','Help','Leave']
                assert paused_host['results_actions']==['Rematch','Configure','Leave']
                assert paused_friend['results_actions']==['Leave']
                focus(friend[0]); select(1); snapshot(friend,'Help')
                press('Escape'); assert snapshot(friend,'Battle')['paused']
                select(0); time.sleep(.3)
                assert not snapshot(host,'Battle')['paused']
                assert not snapshot(friend,'Battle')['paused']
                focus(friend[0]); press('p'); time.sleep(.3)
                assert snapshot(friend,'Battle')['paused']
                select(2); assert snapshot(friend)['network'] is None
                assert snapshot(host,'Battle')['network']['host']
                select(3); assert snapshot(host)['network'] is None
                import hashlib
                report=dict(passed=True,version=initial['version'],transport='LAN TCP/QUIC',
                    main_footer=initial['main_footer'],host_pause_actions=paused_host['pause_actions'],
                    client_pause_actions=paused_friend['pause_actions'],host_results_actions=paused_host['results_actions'],
                    client_results_actions=paused_friend['results_actions'],game_sha256=hashlib.sha256(args.game.read_bytes()).hexdigest(),
                    checks=['Version shown on main screen','Host-only restart and results actions hidden for client',
                            'Client Help and Resume work','Client Leave routes to leaving rather than restart','Host retains full pause menu'])
                args.report.parent.mkdir(parents=True,exist_ok=True);args.report.write_text(json.dumps(report,indent=2)+'\n')
                print(json.dumps(report,indent=2));return
            if args.single_play:
                select(0);snapshot(host,'Battle')
                from measure_playability import run
                save_playability(run(host,host,focus,key,press,snapshot,lobby_action,single_play=True))
                return
            select(1); snapshot(host,'Multiplayer'); select(0); snapshot(host,'Lan')
            assert 'CopyInvite' not in snapshot(host)['lan']['actions']
            press('Escape'); snapshot(host,'Multiplayer'); press('Escape'); snapshot(host,'Main')
            open_online(host, paste_button=True)
            set_clipboard('stale clipboard')
            select(2); copied=clipboard()
            assert parse_invitation(copied)==parse_invitation(invitation), 'Copy server link did not update the OS clipboard'
            select(0); time.sleep(.5)
            state_host=snapshot(host,'NetworkLobby'); assert state_host['network']['id']==1
            assert 'Ready' not in state_host['network']['actions']
            assert 'CopyInvite' not in state_host['network']['actions']
            lobby_action(host, 'Start'); snapshot(host, 'NetworkLobby')
            profile=host[1]/'config/atomic-bomberman/connections.json'; assert profile.exists()
            assert profile.stat().st_mode&0o777==0o600
            code=state_host['network']['room']
            friend=launch('friend'); snapshot(friend,'Main'); open_online(friend,link=copied,exercise_clear=True)
            join_discovered(friend,code)
            state_friend=snapshot(friend,'NetworkLobby'); assert state_friend['network']['id']==2
            if args.playability:
                from measure_playability import run
                report=run(host,friend,focus,key,press,snapshot,lobby_action)
                save_playability(report)
                return
            assert not {'Start', 'Rules', 'Ready'} & set(state_friend['network']['actions'])
            # A second keyboard is assigned locally through the actual lobby UI.
            lobby_action(friend, 'Devices'); snapshot(friend,'NetworkDevices'); select(1); press('Escape'); time.sleep(.3)
            focus(host[0]); state_host=snapshot(host,'NetworkLobby')
            assert state_host['network']['lobby']['owners'].count(2)==2
            # Edit both local players, preserving independent per-seat claims.
            lobby_action(friend, 'Colors'); snapshot(friend, 'NetworkColors')
            press('Right'); press('Down'); press('Right'); press('Escape'); time.sleep(.3)
            lobby = snapshot(host, 'NetworkLobby')['network']['lobby']
            assert lobby['colors'][1:3] == [3, 4], lobby['colors']
            assert len(set(lobby['colors'])) == 10
            lobby_action(host, 'Colors'); snapshot(host, 'NetworkColors')
            press('Left'); press('Escape'); time.sleep(.3)
            assert snapshot(friend, 'NetworkLobby')['network']['lobby']['colors'][0] == 9
            # The host changes shared modes/settings/teams without rewriting
            # either computer's saved local settings.
            settings_file = host[1]/'settings.json'
            saved_settings = settings_file.read_bytes() if settings_file.exists() else None
            lobby_action(host, 'Rules'); snapshot(host, 'NetworkRules')
            press('Right')
            select(2); state=snapshot(host, 'NetworkRules')
            assert state['network']['lobby']['config']['team_play']
            select(10); snapshot(host, 'NetworkTeams'); select(0); press('Escape')
            snapshot(host, 'NetworkRules'); select(2); press('Escape'); time.sleep(.3)
            lobby = snapshot(friend, 'NetworkLobby')['network']['lobby']
            assert lobby['config']['arena'] == 'ClassicGreenAcres'
            assert not lobby['config']['team_play']
            assert lobby['config']['players'][0]['team'] == 1
            assert lobby['config']['network_play'] and not lobby['config']['gold_bomberman']
            assert (settings_file.read_bytes() if settings_file.exists() else None) == saved_settings
            assert snapshot(host,'NetworkLobby')['network']['battle_media_ready']
            assert snapshot(friend,'NetworkLobby')['network']['battle_media_ready']
            # Suspend the actual client renderer, as if cold loading takes time.
            # The graphics supervisor is the parent; pause its game child only.
            parent_pid=player_processes[friend[0]].pid
            child_ids=Path(f'/proc/{parent_pid}/task/{parent_pid}/children').read_text().split()
            assert len(child_ids)==1, 'expected one graphics child'
            client_pid=int(child_ids[0]); os.kill(client_pid,signal.SIGSTOP)
            try:
                lobby_action(host,'Start'); time.sleep(1)
                waiting=snapshot(host,'NetworkStarting')
                assert waiting['network']['starting'] and waiting['network']['tick']==0
                held=waiting['simulation']
                time.sleep(1)
                waiting=snapshot(host,'NetworkStarting')
                assert waiting['network']['tick']==0 and waiting['simulation']==held
            finally:
                os.kill(client_pid,signal.SIGCONT)
            deadline=time.monotonic()+30
            while time.monotonic()<deadline:
                started_host=snapshot(host); started_friend=snapshot(friend)
                if started_host['screen']==started_friend['screen']=='Battle': break
                time.sleep(.2)
            assert started_host['screen']==started_friend['screen']=='Battle', 'shared start did not release both renderers'
            # Exercise the real renderer's local input path before the host
            # can echo it through the delayed datagram route.
            deadline=time.monotonic()+15
            while time.monotonic()<deadline:
                before=snapshot(friend,'Battle')
                if before['network']['confirmed_simulation']['phase']=='Playing': break
                time.sleep(.1)
            assert before['network']['confirmed_simulation']['phase']=='Playing'
            network=before['network']; seat=network['lobby']['owners'].index(network['id'])
            initial=network['confirmed_simulation']['players'][seat]['presentation']['render_position']
            direction='Left' if initial['x']>30000 else 'Right'
            probe=friend[2]; response_ms=None; before_confirmation=False
            started=time.monotonic(); key(direction)
            try:
                deadline=started+1
                while time.monotonic()<deadline:
                    probe.unlink(missing_ok=True); key('F12')
                    sampled=time.monotonic()+.3
                    while not probe.exists() and time.monotonic()<sampled: time.sleep(.001)
                    key('F12',False)
                    if probe.exists():
                        value=json.loads(probe.read_text())
                        rendered=value['simulation']['players'][seat]['presentation']['render_position']
                        confirmed=value['network']['confirmed_simulation']['players'][seat]['presentation']['render_position']
                        if rendered!=initial:
                            response_ms=(time.monotonic()-started)*1000
                            before_confirmation=confirmed==initial
                            break
                    time.sleep(.02)
            finally: key(direction,False); key('F12',False)
            assert response_ms is not None, 'Real client renderer did not respond to movement'
            if args.latency_ms>=80:
                assert before_confirmation, 'Movement waited for the authoritative echo'
            press('p'); time.sleep(.4)
            paused_host=snapshot(host,'Battle'); paused_friend=snapshot(friend,'Battle')
            assert paused_host['paused'] and paused_friend['paused']
            # Once paused, the authoritative game state must be byte-for-byte equal.
            assert paused_host['simulation']==paused_friend['simulation']
            assert paused_host['pause_actions']==['Resume','Help','Restart','Leave']
            assert paused_friend['pause_actions']==['Resume','Help','Leave']
            assert paused_host['results_actions']==['Rematch','Configure','Leave']
            assert paused_friend['results_actions']==['Leave']
            focus(friend[0]); select(1); snapshot(friend,'Help')
            press('Escape'); assert snapshot(friend,'Battle')['paused']
            # The first remaining client row still resumes the shared game.
            select(0); time.sleep(.3)
            assert not snapshot(host,'Battle')['paused']
            assert not snapshot(friend,'Battle')['paused']
            # Pause again so the original host controls remain available.
            press('p'); time.sleep(.3)
            assert snapshot(host,'Battle')['paused']
            # Host returns to lobby after unpausing and ending/restarting through UI.
            focus(host[0]); press('Return'); time.sleep(.2)
            focus(friend[0]); press('Escape'); time.sleep(.3)
            # Escape requests shared pause. The host's leave option closes the room.
            focus(host[0]); select(3); time.sleep(.6)
            assert snapshot(friend)['network'] is None
            assert snapshot(host)['network'] is None
            # Both computers reuse their saved group, with the former client hosting.
            open_online(friend); select(0); time.sleep(.4)
            code=snapshot(friend,'NetworkLobby')['network']['room']
            open_online(host); join_discovered(host,code)
            assert snapshot(host,'NetworkLobby')['network']['id']==2
            assert snapshot(friend,'NetworkLobby')['network']['host']
            lobby_action(friend,'Start'); snapshot(friend,'Battle'); snapshot(host,'Battle')
            focus(host[0]); press('p'); time.sleep(.4)
            assert snapshot(host,'Battle')['pause_actions']==['Resume','Help','Leave']
            select(2); assert snapshot(host)['network'] is None
            assert snapshot(friend,'Battle')['network']['host']
            select(3); assert snapshot(friend)['network'] is None
            for log in logs: log.flush()
            for file in base.rglob('*.log'):
                text=file.read_text()
                assert invitation not in text
            report={'passed':True,'platform':'Ubuntu 24.04 x86-64, isolated Xvfb, Mesa software rendering',
                    'injected_gameplay_round_trip_ms':args.latency_ms,'movement_response_ms':response_ms,'movement_visible_before_confirmation':before_confirmation,'relay_environment':'deployed server, external TCP and UDP 443' if args.live_invite_stdin else 'disposable local TLS relay',
                    'checks':['Main screen displays the compiled game version','Network client pause menu retains Resume, Help and Leave','Network client results retain Leave and hide host-only actions','Client Help, Resume and Leave still route to the correct actions','Multiplayer directly below Start local battle','simple LAN browser and back navigation','Paste server link button reads the OS clipboard','Ctrl-V reads the OS clipboard','Copy server link updates the OS clipboard',
                              'Backspace and Delete clear the whole private link','Invalid and empty pastes clear stale input','Typing cannot silently change a hidden private link','A link copied by one game authenticates another game',
                              'create room','private profile mode 0600',
                              'LAN and game lobbies have no Copy Invite','friend joins through automatic discovery',
                              'multiple local keyboards/controllers in lobby','host-only Start and shared arena/rules/team editing',
                              'start after controls without added Ready toggle','personal colors for both local players and host',
                              'ten unique colors synchronized to both computers','session settings preserve saved local settings',
                              'original network flags disable Gold roulette','actual two-window battle',
                              'announced arena media prepared in both lobbies','slow client holds host countdown at tick zero','all renderers enter the shared countdown automatically',
                              'actual rendered movement responds before the delayed authoritative echo','shared pause and identical serialized simulation','host departure ends session',
                              'saved server reuse','former client hosts without new invitation','credentials absent from logs'],
                    'limitations':['Movement timing uses the window render-state probe, including polling and file I/O; it does not measure display scan-out.','No physical controller tested.','macOS Cmd-V and Linux/macOS WAN play require Mac hardware.']}
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
