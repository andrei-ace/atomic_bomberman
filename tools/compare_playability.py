#!/usr/bin/env python3
"""Repeat actual game-window measurements on the real relay, restoring it after every candidate run.

The isolated candidate listener reuses the production certificate and access
token. Invitations only pass in memory/stdin; reports never contain them.
"""
import argparse
import json
from pathlib import Path
import shlex
import statistics
import subprocess
import re

ROOT = Path(__file__).resolve().parents[1]
APP_VERSION = re.search(r'version = "([^"\n]+)"', (ROOT / 'Cargo.toml').read_text())[1]


def metrics(cases):
    selected = {
        'movement_p95_ms': ('movement', 'key_to_local_frame_ms'),
        'movement_draw_ready_p95_ms': ('movement', 'key_to_local_frame_draw_ready_ms'),
        'stop_p95_ms': ('movement', 'release_to_last_motion_ms'),
        'backward_after_release_p95_pixels': ('movement', 'backward_after_release_pixels'),
        'bomb_p95_ms': ('bomb', 'key_to_local_frame_ms'),
        'action_p95_ms': ('action', 'action_to_local_flame_ms'),
    }
    return dict(
        **{name: statistics.median(case['metrics'][group][field]['p95'] for case in cases)
           for name, (group, field) in selected.items()},
        missed_actions=sum(case['missed_actions'] for case in cases),
        late_dodge_rejections=sum(case['late_dodge_rejections'] for case in cases),
        all_cases_passed=all(case['passed'] for case in cases),
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ssh-host', required=True)
    parser.add_argument('--trial-root', required=True)
    parser.add_argument('--container', default='atomic-bomberman-linux-build')
    parser.add_argument('--display', default=':92')
    parser.add_argument('--repetitions', type=int, default=3)
    parser.add_argument('--label', default='r3')
    parser.add_argument('--report', type=Path, required=True)
    args = parser.parse_args()
    if args.repetitions < 1 or not args.trial_root.startswith('/var/tmp/atomic-authority-wan.'):
        parser.error('Positive repetitions and an existing isolated trial directory are required')
    if not args.label.replace('-', '').isalnum():
        parser.error('Label must contain only letters, numbers and hyphens')
    ssh = ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=8', args.ssh_host]
    trial_script = (ROOT / 'tools/relay_trial.sh').read_bytes()

    def trial(action):
        subprocess.run(ssh + [f'bash -s -- {shlex.quote(action)} {shlex.quote(args.trial_root)}'],
                       input=trial_script, check=True)

    invitation = subprocess.check_output(ssh + [
        'sudo -u bomberman-relay /usr/local/lib/bomberman-relay/bomberman-relay '
        'export-invite --state /var/lib/bomberman-relay/private'])
    report = dict(complete=False, candidate_passed=False, repetitions=args.repetitions,
                  environment='Actual game windows and OS keys over the real public relay',
                  baseline=[], candidate=[], limitations=[
                      'Sequential WAN runs observe changing Internet conditions.',
                      'Both players share a Linux test machine; physical display latency is not measured.'])
    cases = {'baseline': [], 'candidate': []}
    args.report.parent.mkdir(parents=True, exist_ok=True)

    def save():
        serialized = json.dumps(report, indent=2) + '\n'
        assert invitation.decode().strip() not in serialized
        args.report.write_text(serialized)

    save()
    trial('stop')
    for repetition in range(1, args.repetitions + 1):
        order = ('baseline', 'candidate') if repetition % 2 else ('candidate', 'baseline')
        for build in order:
            version = '0.4.1' if build == 'baseline' else APP_VERSION
            name = f'real-relay-playability-{version}-{args.label}-{repetition}'
            case_path = args.report.parent / f'{name}.json'
            command = ['docker', 'exec', '-i', args.container, 'python3',
                       '/workspace/tools/verify_network_ui.py', '--display', args.display,
                       '--assets', '/workspace/local-assets', '--xclip', '/usr/bin/xclip',
                       '--game', f'/workspace/build/authority-{build}-linux-game',
                       '--expected-version', version,
                       '--live-invite-stdin', '--playability', '--report',
                       str(Path('/workspace') / case_path.resolve().relative_to(ROOT))]
            try:
                if build == 'candidate':
                    trial('start')
                    candidate_invite = subprocess.check_output(ssh + [
                        f'{shlex.quote(args.trial_root)}/target/release/bomberman-relay '
                        f'export-invite --state {shlex.quote(args.trial_root)}/private'])
                    assert candidate_invite.strip() == invitation.strip(), 'Trial changed the saved server identity'
                case_path.unlink(missing_ok=True)
                with (ROOT / 'build' / f'{name}.log').open('wb') as log:
                    completed = subprocess.run(command, input=invitation, stdout=log, stderr=subprocess.STDOUT)
                if not case_path.exists():
                    raise RuntimeError(f'{build} GUI test failed without a report; inspect build/{name}.log')
                case = json.loads(case_path.read_text())
                assert case['version'] == version
                assert completed.returncode == (0 if case['passed'] else 1)
                cases[build].append(case)
                report[build].append(case_path.name)
                report[build + '_metrics'] = metrics(cases[build])
                save()
                print(json.dumps(dict(build=build, run=repetition, **metrics([case]))), flush=True)
            finally:
                if build == 'candidate':
                    trial('stop')
    assert len({case['fixture_sha256'] for group in cases.values() for case in group}) == 1
    assert all(len({case['game_sha256'] for case in group}) == 1 for group in cases.values())
    report['complete'] = True
    report['candidate_passed'] = all(case['passed'] for case in cases['candidate'])
    report['fixture_sha256'] = cases['candidate'][0]['fixture_sha256']
    save()
    return 0 if report['candidate_passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
