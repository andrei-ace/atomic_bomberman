#!/usr/bin/env python3
"""Exercise fullscreen in the real game with Openbox or an existing X11 desktop."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, required=True)
    parser.add_argument('--game', type=Path, default=Path('target/debug/bomberman'))
    parser.add_argument('--report', type=Path, required=True)
    parser.add_argument('--existing-wm', action='store_true', help='Use the current desktop and restore its active window afterward')
    parser.add_argument('--xdotool', default='xdotool')
    args = parser.parse_args()
    environment = dict(os.environ)
    if not args.existing_wm:
        environment.update(LIBGL_ALWAYS_SOFTWARE='1', LP_NUM_THREADS='2')
    checks = []
    processes = []
    original_focus = None
    current_window = None
    report = {'passed': False, 'platform': 'Linux X11, existing desktop and native driver' if args.existing_wm else 'Linux X11, Openbox, Mesa software rendering', 'checks': checks}

    def command(*arguments):
        if arguments[0]=='xdotool': arguments=(args.xdotool,*arguments[1:])
        return subprocess.check_output(arguments, env=environment, text=True, stderr=subprocess.DEVNULL).strip()

    def wait_for(predicate, label, seconds=15):
        deadline = time.monotonic()+seconds
        while time.monotonic()<deadline:
            if predicate(): return
            time.sleep(.1)
        raise AssertionError(label)

    def press(name):
        assert command('xdotool','getwindowfocus')==current_window, 'test game does not own keyboard focus'
        subprocess.run([args.xdotool,'key','--clearmodifiers',name], env=environment, check=True)
        time.sleep(.25)

    with tempfile.TemporaryDirectory(prefix='atomic fullscreen ') as temporary:
        private = Path(temporary)
        settings = private/'settings.json'
        environment['XDG_CONFIG_HOME'] = str(private/'config')
        with (private/'wm.log').open('w') as wm_log, (private/'game.log').open('w') as game_log:
            try:
                original_focus = command('xdotool','getactivewindow') if args.existing_wm else None
                if not args.existing_wm:
                    wm = subprocess.Popen(['openbox','--sm-disable'],env=environment,stdout=wm_log,stderr=wm_log)
                    processes.append(wm)
                wait_for(lambda: 'window id' in command('xprop','-root','_NET_SUPPORTING_WM_CHECK'), 'window manager did not start')
                screen = tuple(map(int,command('xdotool','getdisplaygeometry').split()))

                def launch(mode):
                    nonlocal current_window
                    game_log.flush()
                    before = set(subprocess.run([args.xdotool,'search','--onlyvisible','--class','^miniquad-application$'],env=environment,text=True,capture_output=True).stdout.splitlines())
                    game = subprocess.Popen([str(args.game.resolve()),'--assets',str(args.assets.resolve()),
                        '--settings',str(settings),'--mute',mode],env=environment,stdout=game_log,stderr=game_log)
                    processes.append(game)
                    found = []
                    def window_ready():
                        if game.poll() is not None: raise AssertionError('game exited before opening its window')
                        result = subprocess.run([args.xdotool,'search','--onlyvisible','--class','^miniquad-application$'],
                                                env=environment,text=True,capture_output=True)
                        candidates = set(result.stdout.splitlines())-before
                        if result.returncode==0 and candidates:
                            found[:] = [candidates.pop()]
                            return True
                        return False
                    wait_for(window_ready,'game window did not appear',90)
                    # Loading screens yield render frames before menu input is active.
                    wait_for(lambda: 'controller(s).' in (private/'game.log').read_text(), 'game did not finish loading',90)
                    time.sleep(.5)
                    # A graphics fallback can replace its failed first window.
                    wait_for(window_ready,'final game window did not appear')
                    current_window = found[0]
                    subprocess.run([args.xdotool,'windowactivate',current_window],env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
                    subprocess.run([args.xdotool,'windowfocus','--sync',current_window],env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,check=True,timeout=5)
                    return game,found[0]

                def geometry(window):
                    values=dict(line.split('=',1) for line in command('xdotool','getwindowgeometry','--shell',window).splitlines())
                    return int(values['WIDTH']),int(values['HEIGHT'])

                def fullscreen(window):
                    return '_NET_WM_STATE_FULLSCREEN' in command('xprop','-id',window,'_NET_WM_STATE')

                def check_mode(window, expected, label, normal):
                    wait_for(lambda: fullscreen(window)==expected and geometry(window)==(screen if expected else normal),label)
                    assert json.loads(settings.read_text())['fullscreen']==expected
                    assert command('xdotool','getwindowfocus')==window, 'Fullscreen change lost the game keyboard focus'
                    checks.append(label)

                game,window = launch('--windowed')
                normal = geometry(window)
                report['windowed_dimensions'] = normal
                report['fullscreen_dimensions'] = screen
                assert normal!=screen and not fullscreen(window)
                for cycle in range(3):
                    press('F11'); check_mode(window,True,f'F11 enters fullscreen, cycle {cycle+1}',normal)
                    press('F11'); check_mode(window,False,f'F11 restores the window, cycle {cycle+1}',normal)
                # Settings is fourth on Main; Fullscreen is ninth in Settings.
                for _ in range(3): press('Down')
                press('Return')
                for _ in range(8): press('Down')
                press('Return'); check_mode(window,True,'Settings enters fullscreen',normal)
                press('F11'); check_mode(window,False,'F11 exits fullscreen entered through Settings',normal)
                game.terminate(); game.wait(timeout=10)
                wait_for(lambda: subprocess.run(['xprop','-id',window,'WM_CLASS'],env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL).returncode!=0,
                         'previous graphics child did not close')
                # A fresh launch also exercises the pre-map fullscreen hint.
                game_log.seek(0); game_log.truncate(); game_log.flush()
                game,window = launch('--fullscreen')
                wait_for(lambda: fullscreen(window) and geometry(window)==screen, '--fullscreen startup did not fill the display')
                checks.append('--fullscreen startup fills the display')
                # A window first mapped in fullscreen has no previous windowed
                # rectangle. GNOME chooses its decorated size on first exit.
                press('F11')
                wait_for(lambda: not fullscreen(window) and 0<geometry(window)[0]<screen[0]
                         and 0<geometry(window)[1]<screen[1], 'F11 exits fullscreen requested at startup')
                assert not json.loads(settings.read_text())['fullscreen']
                assert command('xdotool','getwindowfocus')==window
                checks.append('F11 exits fullscreen requested at startup')
                report['passed'] = True
            except Exception as error:
                report['error'] = str(error)
                game_log.flush()
                report['game_log_tail'] = (private/'game.log').read_text()[-2000:]
                report['saved_fullscreen'] = json.loads(settings.read_text()).get('fullscreen') if settings.exists() else None
                if current_window:
                    try:
                        report['last_dimensions'] = geometry(current_window)
                        report['wm_fullscreen'] = fullscreen(current_window)
                    except subprocess.CalledProcessError:
                        pass
            finally:
                for process in reversed(processes):
                    if process.poll() is None: process.terminate()
                    try: process.wait(timeout=10)
                    except subprocess.TimeoutExpired: process.kill(); process.wait()
                if args.existing_wm and original_focus:
                    subprocess.run([args.xdotool,'windowactivate',original_focus],env=environment,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL,timeout=5)
    args.report.parent.mkdir(parents=True,exist_ok=True)
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))
    if not report['passed']: raise SystemExit(1)


if __name__=='__main__': main()
