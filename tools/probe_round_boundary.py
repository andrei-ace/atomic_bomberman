#!/usr/bin/env python3
"""Prove original round clock/cache ordering through its idle callback chain.

Runs unchanged43a508/43a56c/43a6a4, the registered42a191 callback, the
42a62c..42a63b survivor guard and the original round initializers. Hooks supply
only empty Windows input/message queues, a static callback allocation and time.
No evolving Rust state is copied into the original machine.
"""
import argparse
from copy import deepcopy
import hashlib
import json
from pathlib import Path

from compare_whole_match import Engine, Rust
from native_harness import UC_HOOK_CODE
from native_round_setup import init_round

ROOT = Path(__file__).resolve().parents[1]
CALLBACK_NODE = 0x1790000
TIME_ENTRY = 0x1791000


def platform_boundaries(engine):
    observations = {'callbacks': 0, 'stop_calls': 0}

    def callback():
        observations['callbacks'] += 1

    def stop():
        observations['stop_calls'] += 1

    additions = {
        0x43b5ec: lambda: engine.ret(),  # Empty Windows message queue.
        0x444f4c: lambda: engine.ret(3),  # Original no-cursor-work mode.
        0x43de70: lambda: engine.ret(-1),  # No keyboard event.
        0x433cac: lambda: engine.ret(-1),  # No controller event.
        0x4444b0: lambda: engine.ret(CALLBACK_NODE),
        TIME_ENTRY: lambda: engine.ret(engine.now),
        0x42a191: callback,  # Observe, then execute original instructions.
        0x410522: stop,  # Observe, then execute original timer stop.
    }
    previous = set(engine.hooks)
    engine.hooks.update(additions)
    import os
    if not os.environ.get('ATOMIC_WHOLE_TRACE'):
        for address in additions.keys() - previous:
            engine.uc.hook_add(UC_HOOK_CODE, engine._hook, begin=address, end=address)
    engine.put(0x4a71dc, TIME_ENTRY)  # Imported timeGetTime thunk.
    engine.put(0x45c4e0, 1)  # Active window; skip inactive-window polling.
    engine.put(0x4a37ac, 0)
    engine.put(0x4a37c4, 0)
    engine.run(0x43a68c)  # Original event-queue reset.
    engine.run(0x43a6fc, eax=0x42a191)  # Original callback registration.
    assert engine.get(CALLBACK_NODE + 4) == 0x42a191
    return observations


def snapshot(engine):
    return {
        'now': engine.now,
        'sampled_frame_time': engine.get(0x46496c),
        'elapsed_ms': engine.get(0x464958),
        'frame': engine.get(0x464994),
        'timer_running': engine.get(0x4601b4),
        'timer_epoch': engine.get(0x4601a0),
        'timer_elapsed_ms': engine.get(0x4601b8),
        'remaining_seconds': engine.get(0x4601a4),
        'countdown_ms': engine.get(0x4621e0),
        'survivors': engine.get(0x4621d4),
        'round_groups_remaining': engine.get(0x4621d8),
        'teams': engine.get(0x4621dc),
    }


def guard(engine):
    engine.run(0x42a62c, stop=0x42a63b)


def case(exe, assets, initial, delay, pre_guard=False):
    engine = Engine(exe, assets, initial)
    observed = platform_boundaries(engine)
    engine.now = 1000
    engine.put(0x46496c, engine.now)
    for address in (0x4621d4, 0x4621d8, 0x4621dc):
        engine.put(address, 1)  # Valid terminal cache from the previous round.
    engine.run(0x42a16f, eax=1)  # Original results-entry suspension.
    engine.now += delay
    old_frame = engine.get(0x464994)
    engine.run(0x43a508)
    assert engine.get(0x464994) == old_frame
    assert engine.get(0x46496c) == 1000
    init_round(engine)
    initialized = snapshot(engine)
    assert initialized['timer_running'] == 1
    assert initialized['survivors'] == 1
    assert initialized['sampled_frame_time'] == 1000
    engine.now += 50
    if pre_guard:
        guard(engine)
    engine.run(0x43a508)
    guard(engine)
    first = snapshot(engine)
    engine.now += 50
    engine.run(0x43a508)
    guard(engine)
    second = snapshot(engine)
    participants = len(initial['simulation']['players'])
    assert first['survivors'] == participants
    assert first['round_groups_remaining'] == (2 if initial['simulation']['config']['team_play'] else participants), (initial['simulation']['config']['team_play'], first)
    expected_teams = (len({p['config']['team'] for p in initial['simulation']['players']})
                      if initial['simulation']['config']['team_play'] else 0)
    assert first['teams'] == expected_teams, first
    assert first['elapsed_ms'] == min(delay + 50, engine.values[31])
    assert first['frame'] == old_frame + 1
    assert second['elapsed_ms'] == 50
    assert second['frame'] == old_frame + 2
    assert first['countdown_ms'] == max(0, initialized['countdown_ms'] - first['elapsed_ms'])
    assert second['countdown_ms'] == max(0, first['countdown_ms'] - 50)
    if pre_guard:
        assert [first['timer_running'], second['timer_running']] == [0, 0]
        assert [first['timer_elapsed_ms'], second['timer_elapsed_ms']] == [0, 0]
    else:
        assert [first['timer_running'], second['timer_running']] == [1, 1]
        assert [first['timer_elapsed_ms'], second['timer_elapsed_ms']] == [50, 100]
    assert observed['callbacks'] == 3
    return {'participants': participants, 'results_delay_ms': delay,
            'team_play': initial['simulation']['config']['team_play'],
            'artificial_pre_callback_guard': pre_guard,
            'initialized': initialized, 'first': first, 'second': second,
            'observations': observed}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--exe', required=True, type=Path)
    parser.add_argument('--assets', type=Path, default=ROOT / 'local-assets')
    parser.add_argument('--rust', type=Path, default=ROOT / 'target/release/examples/whole_match_fixture')
    parser.add_argument('--report', type=Path, default=ROOT / 'docs/evidence/round-boundary-parity.json')
    args = parser.parse_args()
    rust = Rust(args.rust)
    cases = []
    try:
        for participants in (2, 10):
            for countdown in (0, 1000):
                source = rust.request(dict(op='new', seed=1997, players=participants,
                                           human_players=2, arena=0, countdown_ms=countdown))
                for team_play in (False, True):
                    initial = deepcopy(source)
                    initial['simulation']['config']['team_play'] = team_play
                    for slot, player in enumerate(initial['simulation']['players']):
                        player['config']['team'] = slot % 2
                        initial['simulation']['config']['players'][slot]['team'] = slot % 2
                    for delay in (0, 6000):
                        for pre_guard in (False, True):
                            cases.append(case(args.exe, args.assets, initial, delay, pre_guard))
    finally:
        rust.close()
    report = {
        'passed': True,
        'target_sha256': hashlib.sha256(args.exe.read_bytes()).hexdigest(),
        'cases': cases,
        'original_path': ['42a486 calls43a508', '43a508 calls43a56c',
                          '43a56c calls43a6a4', '43a6a4 calls registered42a191',
                          '42a62c calls421969 then410522 only if count<=1'],
        'finding': 'Survivor caches stay stale during round initialization; the first idle callback refreshes them before the outer survivor guard. Running the guard first freezes the newly restarted timer permanently.',
        'limitations': [
            'Empty Windows input/message queues and time are supplied through declared platform hooks.',
            'The results UI is represented by its original battle-suspend call and elapsed wall time; its artwork, event handling and scoring presentation are outside this probe.',
            'The full unchanged engine callback executes; Rust supplies only source configuration through the initial fixture.'
        ],
    }
    args.report.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': True, 'cases': len(cases), 'report': str(args.report)}, indent=2))


if __name__ == '__main__':
    main()
