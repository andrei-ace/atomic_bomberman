#!/usr/bin/env python3
"""Shadow production battle AI decisions with the original x86 dispatcher."""
import argparse
import json
from pathlib import Path
import subprocess

from compare_ai import Original, SHA256


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--exe', type=Path, required=True)
    parser.add_argument('--rust', type=Path, default=Path('target/debug/examples/match_ai_fixture'))
    parser.add_argument('--assets', type=Path, default=Path('local-assets'))
    parser.add_argument('--frames', type=int, default=120)
    parser.add_argument('--report', type=Path, default=Path('docs/evidence/match-ai-parity.json'))
    args = parser.parse_args()
    original = Original(args.exe.read_bytes())
    count = 0
    with subprocess.Popen([str(args.rust.resolve()), str(args.assets.resolve()), str(args.frames)],
                          stdout=subprocess.PIPE, text=True) as process:
        for line in process.stdout:
            trace = json.loads(line)
            native = original.execute(trace['fixture'])
            translated = trace['result']
            for field in ['seed','player','state']:
                if native[field] != translated[field]:
                    Path('.dev/match-ai-failure.json').write_text(json.dumps(trace,indent=2)+'\n')
                    raise AssertionError(f'Production AI decision {count} differs in {field}; .dev/match-ai-failure.json')
            count += 1
            if count % 1000 == 0:
                print(f'Compared {count} production dispatcher calls', flush=True)
        assert process.wait() == 0, 'Production fixture runner failed'
    report = dict(passed=True, production_ai_decisions=count, arenas=11,
        frames_per_arena=args.frames, elapsed_schedule_ms=[16,33,50,75,150],
        target_sha256=SHA256, compared=['actual slot-by-slot production dispatcher decisions',
            'all actor bytes','all persistent AI bytes','shared RNG after each dispatcher'],
        coverage=['ten participants','human and bot opponents','teams and individual play',
                  'real production danger grids','bombs, deaths, hazards and powerups'],
        limitations=['Original dispatcher runs against each production snapshot; complete original match dynamics are not emulated.'])
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report,indent=2))


if __name__=='__main__':
    main()
