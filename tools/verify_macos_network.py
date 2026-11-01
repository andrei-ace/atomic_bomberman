#!/usr/bin/env python3
"""Verify two native macOS windows through LAN and an isolated TLS/QUIC relay.

The injected driver delivers Cocoa events only to each game's own view. Settings,
saved servers, relay credentials, probes, and screenshots use fresh directories.
No Accessibility permission, global input injection, or clipboard is used.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib

from verify_relay import parse_invitation


KEYS = {'Return': 36, 'Escape': 53, 'Down': 125, 'Up': 126,
        'Left': 123, 'Right': 124, 'A': 0, 'C': 8, 'O': 31, 'P': 35,
        'T': 17, '0': 29, 'F1': 122, 'Delete': 117}
ROOT = Path(__file__).resolve().parents[1]


def write_json(path, value, private=False):
    temporary = path.with_suffix(path.suffix + '.tmp')
    temporary.write_text(json.dumps(value, indent=2) + '\n')
    if private:
        temporary.chmod(0o600)
    temporary.replace(path)


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()


def preview_digest(path):
    """Hash the native 224x202 preview pixels, independent of PNG compression."""
    data = path.read_bytes()
    assert data[:8] == b'\x89PNG\r\n\x1a\n', 'invalid screenshot PNG'
    at, compressed = 8, bytearray()
    while at < len(data):
        length = struct.unpack('>I', data[at:at + 4])[0]
        kind, payload = data[at + 4:at + 8], data[at + 8:at + 8 + length]
        if kind == b'IHDR':
            width, height, depth, color, compression, filtering, interlace = struct.unpack('>IIBBBBB', payload)
        elif kind == b'IDAT':
            compressed.extend(payload)
        at += length + 12
    assert (width, height) == (640, 480), (width, height)
    assert depth == 8 and color in (2, 6) and interlace == 0, 'unsupported screenshot PNG format'
    channels = 4 if color == 6 else 3
    stride = width * channels
    raw, previous, cropped = zlib.decompress(compressed), bytearray(stride), bytearray()
    for y in range(height):
        offset = y * (stride + 1)
        mode, row = raw[offset], bytearray(raw[offset + 1:offset + stride + 1])
        for x in range(stride):
            left = row[x - channels] if x >= channels else 0
            above, upper_left = previous[x], previous[x - channels] if x >= channels else 0
            if mode == 1:
                predictor = left
            elif mode == 2:
                predictor = above
            elif mode == 3:
                predictor = (left + above) // 2
            elif mode == 4:
                estimate = left + above - upper_left
                distances = (abs(estimate - left), abs(estimate - above), abs(estimate - upper_left))
                predictor = (left, above, upper_left)[distances.index(min(distances))]
            else:
                assert mode == 0, f'PNG filter {mode}'
                predictor = 0
            row[x] = (row[x] + predictor) & 255
        if 80 <= y < 282:
            cropped.extend(row[378 * channels:602 * channels])
        previous = row
    return hashlib.sha256(cropped).hexdigest()


class Player:
    def __init__(self, role, directory, app, driver, assets, scheme, invitation=None, preserve_settings=False, mod_path=None, extra_environment=None, settings_path=None):
        self.role, self.directory = role, directory
        self.assets = assets
        directory.mkdir(exist_ok=preserve_settings)
        self.command, self.ack, self.probe = (directory / name for name in ('command.json', 'ack.json', 'state.json'))
        self.sequence = self.captures = 0
        self.prefix = 'reload-' if preserve_settings else ''
        for path in (self.command, self.ack, self.probe):
            path.unlink(missing_ok=True)
        self.pid = None
        self.last_png = None
        self.log = directory / f'{self.prefix}game.log'
        self.executable = app / 'Contents/MacOS/bomberman'
        config = directory / 'config'
        profile = config / 'atomic-bomberman'
        profile.mkdir(parents=True, exist_ok=preserve_settings)
        settings = self.settings = settings_path or directory / 'settings.json'
        if not preserve_settings and not (settings_path and settings.exists()):
            write_json(settings, {
            'nickname': 'Native Host' if role == 'host' else 'Native Friend',
            'participant_count': 2,
            'participants': [{'device': 'Keyboard1', 'team': 0, 'enabled': True},
                             {'device': 'Bot', 'team': 1, 'enabled': False}],
            'scheme': scheme, 'random_starts': False, 'sudden_death': False,
            'round_seconds': 150, 'gold': False, 'fullscreen': False,
            })
        else:
            assert settings.is_file(), 'restart requires the existing saved settings'
        if invitation and not preserve_settings:
            write_json(profile / 'connections.json', invitation['server'], private=True)
        environment = {
            'DYLD_INSERT_LIBRARIES': str(driver),
            'XDG_CONFIG_HOME': str(config),
            'ATOMIC_BOMBERMAN_STATE_PROBE': str(self.probe),
            'ATOMIC_BOMBERMAN_MAC_NETWORK_COMMAND': str(self.command),
            'ATOMIC_BOMBERMAN_MAC_NETWORK_ACK': str(self.ack),
            'ATOMIC_BOMBERMAN_MAC_NETWORK_ROLE': role,
        }
        environment.update(extra_environment or {})
        command = ['/usr/bin/open', '-g', '-W', '-n']
        for key, value in environment.items():
            command += ['--env', f'{key}={value}']
        command += ['--stdout', str(self.log), '--stderr', str(self.log), '-a', str(app),
                    '--args', '--assets', str(assets), '--settings', str(settings),
                    '--skip-intro', '--windowed', '--mute', '--seed', '1997']
        if mod_path:
            command += ['--mod', str(mod_path)]
        self.process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            self.wait_ack(0, timeout=60)
            self.wait(lambda state: state['screen'] == 'Main', 'opening main menu')
        except Exception:
            self.close()
            raise

    def wait_ack(self, sequence, timeout=35):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if self.ack.exists():
                value = json.loads(self.ack.read_text())
                if value['id'] == sequence:
                    assert value['ok'], value
                    self.pid = value['pid']
                    assert value['window_visible'], value
                    return value
            if self.process.poll() is not None:
                raise AssertionError(f'{self.role} exited: {self.log.read_text()[-2000:]}')
            time.sleep(.02)
        raise AssertionError(f'{self.role} command {sequence} timed out: {self.log.read_text()[-2000:]}')

    def begin(self, operation, **arguments):
        self.sequence += 1
        write_json(self.command, {'id': self.sequence, 'op': operation, **arguments})
        return self.sequence

    def send(self, operation, **arguments):
        return self.wait_ack(self.begin(operation, **arguments))

    def tap(self, name):
        self.send('tap', code=KEYS[name])

    def click(self, x, y):
        self.send('click', x=x, y=y)

    def capture_begin(self, label=None):
        self.captures += 1
        name = f'{self.prefix}{self.captures:03d}-{label or "poll"}'
        png = self.directory / f'{name}.png' if label else None
        self.last_png = png
        self.capture_path = self.directory / f'{name}.json'
        return self.begin('capture', **({'png': str(png)} if png else {}))

    def capture_end(self, sequence):
        self.wait_ack(sequence)
        state = json.loads(self.probe.read_text())
        write_json(self.capture_path, state)
        return state

    def snapshot(self, label=None):
        return self.capture_end(self.capture_begin(label))

    def wait(self, predicate, reason, timeout=40):
        deadline, state = time.monotonic() + timeout, None
        while time.monotonic() < deadline:
            state = self.snapshot()
            if predicate(state):
                return state
            if (not state.get('network') and state.get('message')
                    and 'Gameplay connection unavailable' in state['message']):
                raise AssertionError(f'{self.role}: {reason}: {state["message"]}')
            time.sleep(.1)
        raise AssertionError(f'{self.role}: {reason}: screen={state["screen"]}, message={state.get("message")}')

    def select(self, index, count, activate=True):
        state = self.snapshot()
        distance = (index - state['selection']) % count
        for _ in range(distance):
            self.tap('Down')
        if activate:
            self.tap('Return')

    def close(self):
        if self.process.poll() is None:
            try:
                self.send('quit')
                self.process.wait(timeout=5)
            except (AssertionError, subprocess.TimeoutExpired, OSError):
                owned = [self.pid] if self.pid else []
                if not owned:
                    # Only this newly copied executable plus its unique fresh
                    # settings path qualify if injection failed before its ACK.
                    processes = subprocess.check_output(['/bin/ps', '-axo', 'pid=,args='], text=True)
                    for line in processes.splitlines():
                        pid, _, arguments = line.strip().partition(' ')
                        if str(self.executable) in arguments and str(self.settings) in arguments:
                            owned.append(int(pid))
                for pid in owned:
                    try:
                        os.kill(pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                self.process.terminate()
                try:
                    self.process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    self.process.kill()
        if self.process.stderr:
            self.process.stderr.close()


def snapshots(host, client, label=None, client_delay=0):
    first, second = (host, client) if client_delay >= 0 else (client, host)
    pending = {first: first.capture_begin(label)}
    if client_delay:
        time.sleep(abs(client_delay))
    pending[second] = second.capture_begin(label)
    return [player.capture_end(pending[player]) for player in (host, client)]


def shared(host, client, phase, label):
    def ready(state):
        network = state.get('network') or {}
        return state['screen'] == phase and network.get('id') and network.get('lobby')
    host.wait(ready, f'host enters {phase}')
    client.wait(ready, f'client follows {phase}')
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        states = snapshots(host, client, label)
        if states[0]['network']['lobby'] == states[1]['network']['lobby']:
            return states
    raise AssertionError(f'{phase}: host/client lobby metadata differ')


def browser_data(state, transport):
    data = state['lan' if transport == 'lan' else 'browser']
    return data, len(data['actions']) + len(data['games' if transport == 'lan' else 'rooms'])


def open_browser(player, transport):
    state = player.snapshot()
    assert state.get('network_menu_layout', 0) >= 2, 'requires the direct LAN/Internet menu build'
    expected = 'Lan' if transport == 'lan' else 'Internet'
    if state['screen'] == 'Main':
        player.select(1 if transport == 'lan' else 2, 7)
    state = player.wait(lambda value: value['screen'] == expected, f'direct {expected} entry')
    if transport == 'relay':
        state = player.wait(lambda value: value['screen'] == expected and value['browser']['ready'], 'saved private server authentication')
    return state


def browser_action(player, transport, action):
    state = player.snapshot()
    data, count = browser_data(state, transport)
    player.select(data['actions'].index(action), count)


def open_transport(player, host, transport, room=None):
    state = open_browser(player, transport)
    if host:
        browser_action(player, transport, 'Host')
        return player.wait(lambda value: value['screen'] == 'NetworkLobby'
                           and value.get('network') and value['network'].get('lobby')
                           and value['network']['lobby'].get('setup'), 'host lobby')
    if transport == 'lan':
        browser_action(player, transport, 'JoinIp')
        state = player.wait(lambda value: value['lan']['manual'], 'manual IP form')
        if state['lan'].get('address'):
            assert state['lan']['address'] == '127.0.0.1:1997', 'last manual LAN address was not restored'
        else:
            player.send('type', text='127.0.0.1')
        player.tap('Return')
    else:
        state = player.wait(lambda value: value['screen'] == 'Internet' and value['browser']['ready']
                            and any(not room or item['code'] == room for item in value['browser']['rooms']), 'local relay room discovery')
        data, count = browser_data(state, transport)
        index = next(i for i, item in enumerate(data['rooms']) if not room or item['code'] == room)
        player.select(len(data['actions']) + index, count)
    return player.wait(lambda state: state['screen'] == 'NetworkLobby' and state.get('network')
                       and state['network'].get('lobby')
                       and len(state['network']['lobby']['peers']) == 2
                       and str(state['network']['id']) in state['network']['lobby']['setup']['nicknames'], 'joined shared lobby')


def read_settings(player):
    return json.loads(player.settings.read_text())


def local_rules(settings):
    return {key: value for key, value in settings.items()
            if key not in ('nickname', 'network_devices', 'network_colors', 'hosting', 'lan_address')}


def verify_help(player, title, return_screen):
    player.tap('F1')
    state = player.wait(lambda value: value['screen'] == 'Help', 'current in-game help')
    assert state['frontend']['document_title'] == title, state['frontend']['document_title']
    player.snapshot('current-help')
    player.tap('Escape')
    player.wait(lambda value: value['screen'] == return_screen, 'help returns to caller')


def configure_browser(player, transport):
    screen = 'Lan' if transport == 'lan' else 'Internet'
    open_browser(player, transport)
    verify_help(player, 'LAN and Internet play', screen)
    browser_action(player, transport, 'Nickname')
    player.wait(lambda value: value['screen'] == 'Nickname', 'global nickname form')
    nickname = f'Native {"Host" if player.role == "host" else "Friend"} Saved'
    player.tap('Delete')
    player.send('type', text=nickname)
    player.snapshot('nickname-form')
    player.tap('Return')
    player.wait(lambda value: value['screen'] == screen and value['frontend']['nickname'] == nickname, 'nickname save and return')
    assert read_settings(player)['nickname'] == nickname
    browser_action(player, transport, 'Nickname')
    player.tap('Delete')
    player.send('type', text='Cancelled name')
    player.tap('Escape')
    player.wait(lambda value: value['screen'] == screen, 'nickname cancel and return')
    assert read_settings(player)['nickname'] == nickname
    before = local_rules(read_settings(player))
    browser_action(player, transport, 'Controls')
    player.wait(lambda value: value['screen'] == 'NetworkDevices', 'browser local controls')
    verify_help(player, 'Player controls', 'NetworkDevices')
    player.select(1, 13)  # Add and remove Keyboard 2; save a distinct network selection.
    player.select(1, 13)
    player.click(100, 416)
    player.wait(lambda value: value['screen'] == screen, 'independent controls Done returns to browser')
    assert read_settings(player)['network_devices'] == ['Keyboard1']
    assert local_rules(read_settings(player)) == before
    player.snapshot('transport-browser')
    return before


def lobby_count(state):
    return state.get('network_lobby_rows', 22) + len(state['network']['actions'])


def lobby_action(player, action):
    state = player.snapshot()
    player.select(state.get('network_lobby_rows', 22) + state['network']['actions'].index(action), lobby_count(state))


def verify_host_preferences(host, client, initial_scheme):
    state = host.snapshot()
    host.select(0, lobby_count(state), activate=False)
    host.tap('Right')
    host.select(1, lobby_count(state), activate=False)
    host.tap('T')
    if host.snapshot()['network']['lobby']['config']['players'][1]['team'] != 0:
        host.select(2, lobby_count(state), activate=False)
        host.tap('T')
    lobby_action(host, 'Options')
    host.wait(lambda value: value['screen'] == 'NetworkOptions', 'optional host Options')
    host.select(0, 10, activate=False)
    schemes = json.loads((host.assets / 'manifest.json').read_text())['schemes']
    target = next(i for i, item in enumerate(schemes) if item['id'].upper().endswith('CHICANE.SCH'))
    for _ in range((target - initial_scheme) % len(schemes)):
        host.tap('Right')
    state = host.snapshot('exact-scheme-filename')
    assert state['network']['lobby']['setup']['scheme_file'] == 'CHICANE.SCH'
    # CHICANE and ASYLUM share a display title but have different layouts.
    # The selected filename must reach clients independently of that title.
    lobby_action(client, 'Options')
    client.wait(lambda value: value['screen'] == 'NetworkOptions' and value['network']['lobby']['setup']['scheme_file'] == 'CHICANE.SCH', 'exact host scheme filename reaches client Options')
    client.snapshot('shared-exact-scheme-filename')
    client.select(9, 10)
    host.select(3, 10, activate=False)
    for _ in range(3):
        host.tap('Right')
    host.snapshot('one-optional-scheme-file-choice')
    host.select(9, 10)
    shared(host, client, 'NetworkLobby', 'host-options-saved')
    host.tap('Return')
    shared(host, client, 'NetworkRules', 'two-row-arena')
    host.tap('Down')
    host.tap('Right')
    host.tap('Escape')
    states = shared(host, client, 'NetworkLobby', 'host-rules-saved')
    config = states[0]['network']['lobby']['config']
    assert (config['wins_to_match'], config['enclosement_depth'], config['team_play']) == (3, 3, True)
    assert config['players'][0]['team'] == 1
    saved = read_settings(host)['hosting']
    assert (saved['wins'], saved['enclosement_depth'], saved['teams']) == (3, 3, True)
    assert saved['scheme'] != initial_scheme
    assert states[0]['network_rule_actions'] == ['Arena', 'VictoryTarget']


def claim_player(player, other, slot):
    state = player.snapshot()
    player.select(slot + 1, lobby_count(state), activate=False)
    # The native sequence is OFF -> AI -> KEY0 for host, OFF -> KEY0 for client.
    for _ in range(2 if state['network']['host'] else 1):
        player.tap('Right')
    player.wait(lambda value: value['network']['lobby']['owners'][slot] == value['network']['id'], 'KEY0 claims its colored player row')
    states = shared(player, other, 'NetworkLobby', 'chosen-player-color')
    assert states[0]['network']['lobby']['colors'] == list(range(10))
    assert states[0]['network']['lobby']['owners'][slot] == state['network']['id']
    assert read_settings(player)['network_colors'][0]['color'] == slot


def exercise_paused_departure(host, client, transport, result):
    check = result['checks'].append
    state = open_transport(host, True, transport)
    result['version'], result['protocol'] = state['version'], state['protocol']
    open_transport(client, False, transport, room=state['network']['room'])
    states = shared(host, client, 'NetworkLobby', 'departure-lobby')
    result['host_asset_fingerprint'] = states[0]['asset_fingerprint']
    result['client_asset_fingerprint'] = states[1]['asset_fingerprint']
    if result.get('modded_host'):
        assert result['host_asset_fingerprint'] != result['client_asset_fingerprint']
        check('modded host and unmodded client join despite different effective asset fingerprints')
    host.select(1, lobby_count(states[0]), activate=False)
    host.tap('Left')
    host.wait(lambda value: value['network']['lobby']['setup']['slots'][0] == 'Open', 'host Left turns player OFF without locking its node')
    host.tap('Right')
    host.wait(lambda value: value['network']['lobby']['setup']['slots'][0] == 'Ai', 'host Right selects AI first')
    host.tap('Right')
    host.wait(lambda value: value['network']['lobby']['owners'][0] == value['network']['id'], 'host Right after AI selects KEY0')
    state = client.snapshot()
    own = state['network']['lobby']['owners'].index(state['network']['id'])
    client.select(own + 1, lobby_count(state), activate=False)
    client.tap('Left')
    client.wait(lambda value: value['network']['lobby']['setup']['slots'][own] == 'Open', 'client Left releases its player')
    client.tap('Right')
    client.wait(lambda value: value['network']['lobby']['owners'][own] == value['network']['id'], 'client Right skips AI and restores KEY0')
    check('original network arrows: Left is OFF; host Right is AI then KEY0; client Right skips AI')
    # Deliberately use rows beyond the compact two-player battle roster.
    claim_player(host, client, 4)
    claim_player(client, host, 7)
    check('two local controls occupy sparse original player rows 5 and 8')
    host.tap('Return')
    shared(host, client, 'NetworkRules', 'departure-arena')
    host.tap('Return')
    for player in (host, client):
        player.wait(lambda value: value['screen'] == 'Battle' and value.get('network')
                    and not value['network']['starting']
                    and value['network']['confirmed_simulation']['phase'] == 'Playing',
                    'departure battle begins', timeout=90)
    host.tap('Escape')
    for player in (host, client):
        player.wait(lambda value: value.get('network') and value['network']['confirmed_simulation']['paused'],
                    'Escape pauses both peers')
    check('Escape pauses the shared match')
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        states = snapshots(host, client, 'paused-host-rules-shared')
        canonical = [value['network']['confirmed_simulation'] for value in states]
        if canonical[0] == canonical[1]:
            result['simulation_sha256'] = digest(canonical[0])
            break
    assert canonical[0] == canonical[1], 'paused canonical state differs across asset packs'
    check('host scheme/configuration and canonical simulation match on both clients')
    state = client.snapshot('client-leaves-paused-match')
    client.select(state['pause_actions'].index('Leave'), len(state['pause_actions']))
    browser = 'Lan' if transport == 'lan' else 'Internet'
    client.wait(lambda value: value['screen'] == browser and value.get('network') is None,
                'client Leave returns to its browser')
    host.wait(lambda value: value.get('network') and len(value['network']['lobby']['peers']) == 1,
              'host retains the room after client departure')
    check('client leaves through the pause menu without closing the host session')
    host.tap('Escape')
    state = host.wait(lambda value: value.get('network') and not value['network']['confirmed_simulation']['paused'],
                      'host resumes with Escape')
    resumed_tick = state['network']['tick']
    state = host.wait(lambda value: value.get('network')
                      and value['network']['confirmed_simulation']['players'][1]['config']['control'] == 'Bot',
                      'departed compact battle seat becomes AI', timeout=15)
    assert state['network']['tick'] > resumed_tick
    assert state['network']['host'] and len(state['network']['confirmed_simulation']['players']) == 2
    host.snapshot('host-continues-after-ai-handoff')
    check('host resumes and continues beyond the five-second AI handoff without an invalid-message error')
    deadline = time.monotonic() + 8
    rendered_positions = set()
    moving_seen = False
    while time.monotonic() < deadline:
        state = host.snapshot()
        player = state['simulation']['players'][1]
        pose = player['presentation']
        if player['alive'] and pose['render_visible'] and pose['render_alive']:
            rendered_positions.add((pose['render_position']['x'], pose['render_position']['y']))
            moving_seen |= pose['render_moving']
        if len(rendered_positions) >= 4 and moving_seen:
            break
        time.sleep(.1)
    assert len(rendered_positions) >= 4 and moving_seen, 'AI plays but its displayed avatar is frozen'
    host.snapshot('departed-player-ai-visibly-moving')
    result['ai_rendered_positions'] = len(rendered_positions)
    check('departed player AI visibly moves and uses its walking animation instead of the last client pose')
    host.tap('Escape')
    state = host.wait(lambda value: value.get('network') and value['network']['confirmed_simulation']['paused'],
                      'host pauses surviving match')
    host.select(state['pause_actions'].index('Leave'), len(state['pause_actions']))
    host.wait(lambda value: value['screen'] == browser and value.get('network') is None, 'host leaves explicitly')
    for player in (host, client):
        player.tap('Escape')
        player.wait(lambda value: value['screen'] == 'Main', 'Escape returns to main menu')
        for _ in range(2):
            player.tap('Escape')
        assert player.snapshot('main-escape-keeps-app-open')['screen'] == 'Main'
    check('Escape returns or resumes and never quits from the main menu for either player')


def quit_from_browser(player, transport):
    browser = 'Lan' if transport == 'lan' else 'Internet'
    player.wait(lambda state: state['screen'] == browser and not state.get('network'), 'return to browser before quit')
    player.tap('Escape')
    player.wait(lambda state: state['screen'] == 'Main', 'main menu before quit')
    player.select(6, 7, activate=False)
    player.begin('tap', code=KEYS['Return'])
    player.process.wait(timeout=15)  # Exit runs the game's ordinary settings save.


def exercise_shared_host_settings(host, client, transport, result, players, app, driver, assets, scheme):
    check = result['checks'].append
    assert host.settings == client.settings
    state = open_transport(host, True, transport)
    result['version'], result['protocol'] = state['version'], state['protocol']
    open_transport(client, False, transport, room=state['network']['room'])
    shared(host, client, 'NetworkLobby', 'shared-profile-before-edits')
    verify_host_preferences(host, client, scheme)
    lobby_action(host, 'Options')
    host.wait(lambda state: state['screen'] == 'NetworkOptions', 'shared-profile host options')
    for row in (2, 4, 5, 6):  # Time, random starts, conveyor speed, stomp behavior.
        host.select(row, 10, activate=False)
        host.tap('Right')
    host.snapshot('shared-profile-all-host-options')
    host.select(9, 10)
    shared(host, client, 'NetworkLobby', 'shared-profile-host-rules')
    host.tap('Return')
    shared(host, client, 'NetworkRules', 'shared-profile-arena')
    host.select(0, 2, activate=False)
    host.tap('Left' if transport == 'relay' else 'Right')
    host.select(1, 2, activate=False)
    host.tap('Right')
    host.tap('Right')
    host.tap('Escape')
    states = shared(host, client, 'NetworkLobby', 'shared-profile-final-rules')
    expected = read_settings(host)['hosting']
    assert (expected['wins'], expected['enclosement_depth'], expected['teams']) == (5, 3, True)
    assert (expected['round_seconds'], expected['random_starts'], expected['conveyor_speed'], expected['stomped_bombs_detonate']) == (180, True, 2, False)
    assert expected['random_arena'] == (transport == 'relay')
    config = states[0]['network']['lobby']['config']
    check('two native host/client windows share one settings file loaded before host edits')
    check('scheme, time, wins, teams, arena, enclosure, random starts, conveyor and stomp choices save')
    # Close the host first. The client still holds its older preferences and
    # must not replace the saved host rules when it subsequently saves/quits.
    host.tap('Escape')
    quit_from_browser(host, transport)
    browser = 'Lan' if transport == 'lan' else 'Internet'
    client.wait(lambda state: state['screen'] == browser and not state.get('network'), 'client returns after host leaves')
    browser_action(client, transport, 'Nickname')
    client.wait(lambda state: state['screen'] == 'Nickname', 'edit an unrelated shared preference')
    client.tap('Delete')
    client.send('type', text='Shared profile player')
    client.tap('Return')
    client.wait(lambda state: state['screen'] == browser, 'client nickname saved')
    assert read_settings(client)['hosting'] == expected, 'stale client save replaced host rules'
    quit_from_browser(client, transport)
    assert read_settings(client)['hosting'] == expected, 'closing the client last replaced host rules'
    assert read_settings(client)['nickname'] == 'Shared profile player'
    old_pids = {host.pid, client.pid}
    directories = [host.directory, client.directory]
    settings = host.settings
    players.clear()
    check('client saves an unrelated preference and quits last without overwriting host rules')
    host = Player('host', directories[0], app, driver, assets, scheme, preserve_settings=True, settings_path=settings)
    players.append(host)
    client = Player('client', directories[1], app, driver, assets, scheme, preserve_settings=True, settings_path=settings)
    players.append(client)
    assert not old_pids.intersection({host.pid, client.pid})
    state = open_transport(host, True, transport)
    state = host.wait(lambda value: value['network']['lobby']['setup']['scheme_file'] == 'CHICANE.SCH', 'saved exact scheme restored')
    lobby = state['network']['lobby']
    for key in ('arena', 'wins_to_match', 'win_by_kills', 'round_time_ms', 'team_play', 'random_starts', 'enclosement_depth', 'conveyor_speed', 'stomped_bombs_detonate', 'scheme'):
        assert lobby['config'][key] == config[key], f'reopened host lost {key}'
    assert [p['team'] for p in lobby['config']['players']] == expected['slot_teams']
    assert lobby['setup'].get('random_arena', False) == expected['random_arena']
    assert lobby['setup']['slots'].count('Open') == 9
    open_transport(client, False, transport, room=state['network']['room'])
    shared(host, client, 'NetworkLobby', 'shared-profile-restored-host-options')
    host.tap('Return')
    shared(host, client, 'NetworkRules', 'shared-profile-restored-preview')
    result['saved_hosting'] = expected
    result['shared_settings_verified'] = True
    result['restart_verified'] = True
    check('fresh host/client processes restore all match rules, exact scheme filename, teams and arena preview')


def verify_transport(transport, directory, app, driver, assets, scheme, relay_executable, live_profile=None, paused_departure=False, host_mod=None, host_assets=None, shared_host_settings=False):
    directory.mkdir()
    relay = None
    relay_log = None
    players = []
    result = {'transport': transport, 'passed': False, 'checks': [], 'modded_host': bool(host_mod or host_assets)}
    check = result['checks'].append
    try:
        invitation = None
        if transport == 'relay' and live_profile:
            invitation = {'server': json.loads(live_profile.read_text()), 'room': None}
        elif transport == 'relay':
            with socket.socket() as reserve:
                reserve.bind(('127.0.0.1', 0))
                port = reserve.getsockname()[1]
            relay_state = directory / 'relay-state'
            subprocess.run([str(relay_executable), 'init', '--state', str(relay_state),
                            '--advertise', f'127.0.0.1:{port}', '--listen', f'127.0.0.1:{port}'],
                           check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            private_link = subprocess.check_output([str(relay_executable), 'export-invite',
                                                    '--state', str(relay_state)], text=True).strip()
            invitation = parse_invitation(private_link)
            relay_log = (directory / 'relay.log').open('w')
            relay = subprocess.Popen([str(relay_executable), 'serve', '--state', str(relay_state)],
                                     stdout=relay_log, stderr=relay_log)
            time.sleep(.3)
            assert relay.poll() is None, 'local relay did not start'
        shared_settings = directory / 'shared-settings.json' if shared_host_settings else None
        host = Player('host', directory / 'host', app, driver, host_assets or assets, scheme, invitation, mod_path=host_mod, settings_path=shared_settings)
        players.append(host)
        client = Player('client', directory / 'client', app, driver, assets, scheme, invitation, settings_path=shared_settings)
        players.append(client)
        result['pids'] = [host.pid, client.pid]
        assert host.pid != client.pid
        check('two distinct native game windows sharing preferences with isolated test connections' if shared_host_settings
              else 'two distinct native production game windows with isolated profiles')
        if shared_host_settings:
            exercise_shared_host_settings(host, client, transport, result, players, app, driver, assets, scheme)
            result['passed'] = True
            return result
        if paused_departure:
            exercise_paused_departure(host, client, transport, result)
            result['passed'] = True
            return result
        local_before = {player.role: configure_browser(player, transport) for player in (host, client)}
        check('main entries open LAN and Internet directly with current browser help')
        check('global nicknames save and cancel correctly in the original-style form')
        check('independent network controls save and Done returns to the correct browser')
        state = open_transport(host, True, transport)
        result['version'], result['protocol'] = state['version'], state['protocol']
        setup = state['network']['lobby']['setup']
        assert setup['slots'][0] == {'Human': 1}
        assert setup['slots'][1:] == ['Open'] * 9
        check('unoccupied slots initially Open')
        before_id = state['network']['id']
        host.select(1, lobby_count(state), activate=False)
        host.tap('Left')
        state = host.wait(lambda value: value['network']['lobby']['owners'][0] is None, 'host last player can turn OFF')
        assert state['network']['id'] == before_id and len(state['network']['lobby']['peers']) == 1
        host.tap('Right')
        host.tap('Right')
        host.wait(lambda value: value['network']['lobby']['owners'][0] == before_id, 'host can restore KEY0 after OFF')
        check('host can turn its last player OFF and restore KEY0 without losing the room')
        open_transport(client, False, transport, room=state['network']['room'])
        states = shared(host, client, 'NetworkLobby', 'lobby-open')
        verify_host_preferences(host, client, scheme)
        states = shared(host, client, 'NetworkLobby', 'preferences-shared')
        check('optional Options saves one Scheme File and host rules; preview has only arena/wins')
        lobby = states[0]['network']['lobby']
        assert sorted(lobby['setup']['nicknames'].values()) == ['Native Friend Saved', 'Native Host Saved']
        assert len(lobby['peers']) == 2
        check('connected peer nicknames and shared ten-slot roster')
        if transport == 'lan':
            assert read_settings(client)['lan_address'] == '127.0.0.1:1997'
            check('successful manual LAN address saves')
        # Select empty slot 9 without activating it, then use explicit shortcuts.
        host.select(9, lobby_count(states[0]), activate=False)
        host.tap('A')
        host.select(10, lobby_count(states[0]), activate=False)
        host.tap('C')
        states = shared(host, client, 'NetworkLobby', 'lobby-ai-closed')
        assert states[0]['network']['lobby']['setup']['slots'][8] == 'Ai'
        assert states[0]['network']['lobby']['setup']['slots'][9] == 'Closed'
        check('host can set explicit AI and Closed slots without disconnecting users')
        before = states[0]['network']['lobby']
        client.click(250, 100)
        client.tap('A')
        states = shared(host, client, 'NetworkLobby', 'client-lobby-readonly')
        assert states[0]['network']['lobby'] == before
        check('client cannot alter host slot setup')
        claim_player(host, client, 4)
        claim_player(client, host, 5)
        check('host and client choose colors directly with KEY0 on the player rows; no color menu')
        # Teams belong to player rows too. The host deliberately puts the two
        # selected rows on different teams after moving the local controls.
        state = host.snapshot()
        host.select(6, lobby_count(state), activate=False)
        if state['network']['lobby']['config']['players'][5]['team'] == 0:
            host.tap('T')
        state = client.snapshot()
        client.select(6, lobby_count(state), activate=False)
        client.tap('Left')
        client.wait(lambda value: value['network']['lobby']['owners'][5] is None, 'client turns own player OFF')
        client.tap('Right')
        client.wait(lambda value: value['network']['lobby']['owners'][5] == value['network']['id'], 'client restores KEY0')
        check('client OFF and KEY0 reassignment preserve the node connection')
        state = host.snapshot()
        host.select(6, lobby_count(state), activate=False)
        host.tap('0')
        state = host.wait(lambda value: value['network']['lobby']['owners'][5] is None, 'host overrides a remote assignment with zero')
        assert len(state['network']['lobby']['peers']) == 2
        client.tap('Right')
        shared(host, client, 'NetworkLobby', 'remote-player-reclaimed')
        client.wait(lambda value: value['network']['lobby']['owners'][5] == value['network']['id'], 'client reclaims after override')
        check('host override releases a player without disconnecting its node')
        states = snapshots(host, client)
        host.select(9, lobby_count(states[0]), activate=False)
        host.tap('O')
        host.select(10, lobby_count(states[0]), activate=False)
        host.tap('O')
        # Enter works from a player row; no traversal to a Continue button is needed.
        host.tap('Return')
        states = shared(host, client, 'NetworkRules', 'arena-initial')
        assert states[0]['network']['lobby']['setup']['phase'] == 'Arena'
        assert states[0]['network_rule_actions'] == ['Arena', 'VictoryTarget']
        assert preview_digest(host.last_png) == preview_digest(client.last_png)
        check('players advance directly to the two-row original arena preview without a controls wizard')
        original_arena = states[0]['network']['lobby']['config']['arena']
        host.tap('Right')
        states = shared(host, client, 'NetworkRules', 'arena-changed')
        assert states[0]['network']['lobby']['config']['arena'] != original_arena
        assert preview_digest(host.last_png) == preview_digest(client.last_png)
        check('host arena choice propagates with identical illustrative preview pixels')
        readonly = states[0]['network']['lobby']
        client.tap('Right')
        client.tap('Return')
        states = shared(host, client, 'NetworkRules', 'client-arena-readonly')
        assert states[0]['network']['lobby'] == readonly
        check('client follows the host preview and cannot change or start it')
        host.tap('Escape')
        shared(host, client, 'NetworkLobby', 'back-to-players')
        host.tap('Return')
        shared(host, client, 'NetworkRules', 'arena-again')
        check('Escape returns directly from arena to players; one Enter proceeds again')
        host.tap('Return')
        for player in (host, client):
            player.wait(lambda state: state['screen'] == 'Battle' and state.get('network')
                        and not state['network']['starting'], 'automatic loading barrier/countdown', timeout=90)
        states = snapshots(host, client, 'battle-countdown')
        result['opening_phases'] = [state['network']['confirmed_simulation']['phase'] for state in states]
        assert any(isinstance(phase, dict) and 'Countdown' in phase for phase in result['opening_phases']), 'opening countdown was not observed'
        check('host starts both windows through automatic media loading and ready barrier')
        for player in (host, client):
            player.wait(lambda state: state['screen'] == 'Battle'
                        and state['network']['confirmed_simulation']['phase'] == 'Playing', 'shared countdown completes')
        host.tap('P')
        for player in (host, client):
            player.wait(lambda state: state['network']['confirmed_simulation']['paused'], 'host pause reaches both peers')
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            states = snapshots(host, client, 'paused-state')
            simulations = [state['network']['confirmed_simulation'] for state in states]
            if simulations[0] == simulations[1]:
                break
        assert simulations[0] == simulations[1], 'confirmed paused simulation differs'
        assert len(simulations[0]['players']) == 2
        assert simulations[0]['config'] == simulations[1]['config']
        result['simulation_sha256'] = digest(simulations[0])
        result['config_sha256'] = digest(simulations[0]['config'])
        result['arena'] = simulations[0]['config']['arena']
        check('full confirmed simulation and compact two-player configuration match')
        deadline = time.monotonic() + 10
        engine_match = False
        client_delay = 0
        while time.monotonic() < deadline:
            states = snapshots(host, client, client_delay=client_delay)
            networks = [state['network'] for state in states]
            if networks[0]['tick'] == networks[1]['tick']:
                assert networks[0]['confirmed_hash'] == networks[1]['confirmed_hash']
                result['confirmed_tick'] = networks[0]['tick']
                result['engine_replay_hash'] = networks[0]['confirmed_hash']
                engine_match = True
                break
            client_delay = max(-.25, min(.25, client_delay + .01 * (networks[0]['tick'] - networks[1]['tick'])))
        assert engine_match, 'could not capture both windows at the same committed tick'
        check('exact engine Replay hash matches at the same committed tick')
        for player in (host, client):
            verify_help(player, 'Atomic Bomberman - Game guide', 'Battle')
            assert player.snapshot()['network']['confirmed_simulation']['paused']
        check('current in-game guide opens and returns to the paused battle for both roles')
        saved = {player.role: read_settings(player) for player in (host, client)}
        for player in (host, client):
            assert local_rules(saved[player.role]) == local_before[player.role], 'network UI changed local-game rules'
            assert saved[player.role]['network_devices'] == ['Keyboard1']
            assert len(saved[player.role]['network_colors']) == 1
        assert saved['client']['hosting'] is None, 'client copied host preferences'
        check('network preferences preserve local-game rules and client hosting preferences')
        profile_before = {
            player.role: json.loads((player.directory / 'config/atomic-bomberman/connections.json').read_text())
            for player in (host, client)
        } if transport == 'relay' else None
        directories = {player.role: player.directory for player in (host, client)}
        previous_pids = [player.pid for player in (host, client)]
        for player in reversed(players):
            player.close()
        players.clear()
        host = Player('host', directories['host'], app, driver, host_assets or assets, scheme, preserve_settings=True, mod_path=host_mod)
        players.append(host)
        client = Player('client', directories['client'], app, driver, assets, scheme, preserve_settings=True)
        players.append(client)
        assert all(player.pid not in previous_pids for player in players), 'restart reused a game process'
        state = open_transport(host, True, transport)
        lobby = state['network']['lobby']
        assert (lobby['config']['wins_to_match'], lobby['config']['enclosement_depth'], lobby['config']['team_play']) == (3, 3, True)
        assert lobby['setup']['slots'].count('Open') == 9
        open_transport(client, False, transport, room=state['network']['room'])
        states = shared(host, client, 'NetworkLobby', 'restart-saved-room')
        assert sorted(states[0]['network']['lobby']['setup']['nicknames'].values()) == ['Native Friend Saved', 'Native Host Saved']
        for player, state in zip((host, client), states):
            lobby = state['network']['lobby']
            color = saved[player.role]['network_colors'][0]['color']
            player.wait(lambda value: value['network']['lobby']['owners'][color] == value['network']['id'], 'saved available player row reapplied')
            assert local_rules(read_settings(player)) == local_before[player.role]
            assert read_settings(player)['nickname'] == saved[player.role]['nickname']
            assert read_settings(player)['network_devices'] == ['Keyboard1']
            if profile_before:
                profile = player.directory / 'config/atomic-bomberman/connections.json'
                assert json.loads(profile.read_text()) == profile_before[player.role]
                assert profile.stat().st_mode & 0o777 == 0o600
        if transport == 'lan':
            assert read_settings(client)['lan_address'] == '127.0.0.1:1997'
        states = shared(host, client, 'NetworkLobby', 'restart-persisted-player-rows')
        for state in states:
            lobby = state['network']['lobby']
            for player in (host, client):
                color = saved[player.role]['network_colors'][0]['color']
                # Peer IDs are scoped to the new room, independent of PIDs.
                peer_id = next(value['network']['id'] for value in states if value['network']['host'] == (player.role == 'host'))
                owned = [slot for slot, owner in enumerate(lobby['owners']) if owner == peer_id]
                assert owned and all(lobby['colors'][slot] == color for slot in owned)
        assert read_settings(client)['hosting'] is None
        check('real process restart restores names, devices, colors and separate host rules')
        check('fresh room resets unused slots Open and reuses saved connection preferences')
        host.tap('Return')
        shared(host, client, 'NetworkRules', 'minimal-restart-arena')
        host.tap('Return')
        for player in (host, client):
            player.wait(lambda value: value['screen'] == 'Battle' and value.get('network') and not value['network']['starting'], 'two-confirm minimal start', timeout=90)
        check('saved settings start a fresh room with two confirms: players, arena, battle')
        result['restart_verified'] = True
        result['passed'] = True
    except Exception as error:
        result['error'] = f'{type(error).__name__}: {error}'
        raise
    finally:
        for player in reversed(players):
            player.close()
        if relay:
            relay.terminate()
            try:
                relay.wait(timeout=5)
            except subprocess.TimeoutExpired:
                relay.kill()
                relay.wait()
        if relay_log:
            relay_log.close()
        write_json(directory / 'report.json', result)
    return result


def verify_version_rejection(directory, app, driver, assets, scheme, old_relay):
    directory.mkdir()
    with socket.socket() as reserve:
        reserve.bind(('127.0.0.1', 0))
        port = reserve.getsockname()[1]
    private = directory / 'private'
    subprocess.run([str(old_relay), 'init', '--state', str(private),
                    '--advertise', f'127.0.0.1:{port}', '--listen', f'127.0.0.1:{port}'],
                   check=True, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    link = subprocess.check_output([str(old_relay), 'export-invite', '--state', str(private)], text=True)
    invitation = parse_invitation(link)
    checks = []
    player = None
    with (directory / 'relay.log').open('w') as log:
        relay = subprocess.Popen([str(old_relay), 'serve', '--state', str(private)], stdout=log, stderr=log)
        try:
            time.sleep(.3)
            assert relay.poll() is None
            player = Player('host', directory / 'player', app, driver, assets, scheme, invitation)
            player.click(460, 201)
            state = player.wait(lambda value: bool(value.get('compatibility_error')), 'explicit old-relay error')
            assert state['screen'] == 'Internet' and state['network'] is None
            assert 'protocol 8' in state['compatibility_error']
            checks.append('protocol-8 client rejects the older relay before entering a lobby')
            first_error = state['compatibility_error']
            time.sleep(1)
            assert player.snapshot('persistent-version-error')['compatibility_error'] == first_error
            player.click(100, 110)
            state = player.snapshot('background-input-blocked')
            assert state['network'] is None and state['compatibility_error'] == first_error
            checks.append('compatibility error persists and blocks background Host input')
            player.tap('Escape')
            state = player.wait(lambda value: value.get('compatibility_error') is None, 'dismiss version error')
            assert state['screen'] == 'Internet'
            profile = player.directory / 'config/atomic-bomberman/connections.json'
            assert json.loads(profile.read_text()) == invitation['server']
            checks.append('Escape returns to the browser and preserves the private server profile')
            player.select(5, 7)
            player.wait(lambda value: bool(value.get('compatibility_error')), 'retry incompatible relay')
            checks.append('Refresh explicitly retries and shows the same compatibility error')
            return {'passed': True, 'checks': checks}
        finally:
            if player:
                player.close()
            relay.terminate()
            relay.wait(timeout=10)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--output', type=Path, default=Path('build/transport-menu-network'))
    parser.add_argument('--game', type=Path, default=Path('target/debug/bomberman'))
    parser.add_argument('--relay', type=Path, default=Path('target/debug/bomberman-relay'))
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--transport', choices=('both', 'lan', 'relay'), default='both')
    parser.add_argument('--evidence', type=Path, help='write a sanitized passing report for source control')
    parser.add_argument('--live-profile', type=Path, help='use a private existing relay profile for the relay-only run')
    parser.add_argument('--old-relay', type=Path, help='verify rejection using a preserved older relay executable')
    parser.add_argument('--compatibility-only', action='store_true')
    parser.add_argument('--host-assets', type=Path, help='use an existing mod cache for the host while the client uses --assets')
    parser.add_argument('--host-mod', type=Path, help='run the host with this asset mod while its client uses the unmodded base')
    parser.add_argument('--paused-departure', action='store_true', help='verify pause, client Leave, host resume, AI handoff and safe Escape')
    parser.add_argument('--shared-host-settings', action='store_true', help='verify two windows sharing preferences cannot overwrite saved host rules')
    args = parser.parse_args()
    if sys.platform != 'darwin':
        parser.error('requires a macOS desktop')
    if args.live_profile and args.transport != 'relay':
        parser.error('--live-profile requires --transport relay')
    if args.compatibility_only and not args.old_relay:
        parser.error('--compatibility-only requires --old-relay')
    assets, game, relay = args.assets.resolve(), args.game.resolve(), args.relay.resolve()
    manifest = json.loads((assets / 'manifest.json').read_text())
    scheme = next(i for i, item in enumerate(manifest['schemes']) if item['id'].upper().endswith('BASIC.SCH'))
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    directory = Path(tempfile.mkdtemp(prefix='run-', dir=output))
    if not args.skip_build:
        subprocess.run(['cargo', 'build', '--locked', '-p', 'bomberman', '-p', 'bomberman-relay'], cwd=ROOT, check=True)
    driver = directory / 'network-driver.dylib'
    subprocess.run(['xcrun', 'clang', '-dynamiclib', '-fobjc-arc', '-framework', 'Cocoa',
                    str(ROOT / 'tools/macos-network-driver.m'), '-o', str(driver)], check=True)
    app = directory / 'Atomic Network Check.app'
    executable = app / 'Contents/MacOS/bomberman'
    executable.parent.mkdir(parents=True)
    shutil.copy2(game, executable)
    with (app / 'Contents/Info.plist').open('wb') as info:
        plistlib.dump({'CFBundleIdentifier': 'com.andrei.atomic-bomberman.network-check',
                      'CFBundleName': 'Atomic Bomberman Network Check',
                      'CFBundleExecutable': 'bomberman', 'CFBundlePackageType': 'APPL',
                      'NSHighResolutionCapable': True}, info)
    subprocess.run(['codesign', '--force', '--sign', '-', str(executable)], check=True)
    subprocess.run(['codesign', '--force', '--sign', '-', str(app)], check=True)
    report = {'passed': False, 'evidence_directory': str(directory), 'transports': [],
              'game_binary_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(),
              'driver_sha256': hashlib.sha256(driver.read_bytes()).hexdigest(),
              'assets_manifest_sha256': hashlib.sha256((assets / 'manifest.json').read_bytes()).hexdigest(),
              'backend': 'two native production game windows, process-local Cocoa NSEvents',
              'isolated_settings_and_saved_servers': True, 'clipboard_used': False,
              'shared_settings_between_windows': args.shared_host_settings,
              'limitations': [('Live private relay transport; no physical device or physical audio claims.' if args.live_profile else 'Local loopback transport validation; no deployed WAN or physical device claims.'),
                              'Audio is muted; screenshots capture the production 640x480 game render target.',
                              'Splash timing is checked separately; this harness uses --skip-intro.']}
    try:
        if args.old_relay:
            report['compatibility'] = verify_version_rejection(directory / 'compatibility', app, driver, assets, scheme, args.old_relay.resolve())
        transports = () if args.compatibility_only else ('lan', 'relay') if args.transport == 'both' else (args.transport,)
        for transport in transports:
            try:
                result = verify_transport(transport, directory / transport, app, driver, assets, scheme, relay, args.live_profile, args.paused_departure, args.host_mod, args.host_assets.resolve() if args.host_assets else None, args.shared_host_settings)
            except Exception:
                partial = directory / transport / 'report.json'
                if partial.exists():
                    report['transports'].append(json.loads(partial.read_text()))
                raise
            report['transports'].append(result)
        report['passed'] = True
    except Exception as error:
        report['error'] = f'{type(error).__name__}: {error}'
    write_json(directory / 'report.json', report)
    write_json(output / 'report.json', report)
    if args.evidence and report['passed']:
        sanitized = json.loads(json.dumps(report))
        sanitized.pop('evidence_directory', None)
        for result in sanitized['transports']:
            result.pop('pids', None)
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        write_json(args.evidence, sanitized)
    print(json.dumps(report, indent=2))
    if not report['passed']:
        raise SystemExit(1)


if __name__ == '__main__':
    main()
