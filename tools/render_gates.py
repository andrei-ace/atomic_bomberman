"""Frame-by-frame invariants for the existing controlled GUI playability trial."""
import math


def inspect(local, peer, commands, seat):
    failures = []
    compared = 0
    canonical = {}
    for frame in peer:
        if frame.get('canonical_hash'):
            canonical[(frame.get('generation'), frame['round'], frame['tick'])] = frame['canonical_hash']
    for frame in local:
        key = (frame.get('generation'), frame['round'], frame['tick'])
        if key in canonical and frame.get('canonical_hash'):
            compared += 1
            if canonical[key] != frame['canonical_hash'] and len(failures) < 16:
                failures.append(dict(invariant='canonical_hash', tick=frame['tick'], frame=frame['frame']))
    checked = 0
    for command in commands:
        if command['kind'] != 'movement':
            continue
        previous = None
        idle_since = None
        animation_since = None
        for frame in local:
            if not command['issued_ns'] <= frame['input_observed_ns'] < command['until_ns']:
                continue
            player = frame['players'][seat]
            if previous is None:
                previous = frame
                continue
            old = previous['players'][seat]
            now = frame['input_observed_ns']
            dt_ms = (now-previous['input_observed_ns'])/1e6
            dx = player['position']['x']-old['position']['x']
            dy = player['position']['y']-old['position']['y']
            direction = 'Right' if 'Right' in frame['keys'] else 'Left' if 'Left' in frame['keys'] else None
            old_direction = 'Right' if 'Right' in previous['keys'] else 'Left' if 'Left' in previous['keys'] else None
            invariant = None
            # This trial independently excludes transit, damage and obstacles
            # by its fixed open arena and movement-only input schedule.
            if frame['round'] != previous['round'] or player.get('special', 0) in (5,6,7):
                invariant = 'unexpected_transit_or_round'
            elif direction == old_direction == 'Right' and dx < 0 or direction == old_direction == 'Left' and dx > 0:
                invariant = 'backward_movement'
            elif abs(dx) > (math.ceil(dt_ms/20)+1)*400 or dy:
                invariant = 'movement_speed'
            if direction is None:
                if idle_since is None:
                    idle_since = now
                if now-idle_since >= 40_000_000 and (dx or dy or player['moving']):
                    invariant = 'motion_after_release_or_stale_pose'
            else:
                idle_since = None
            if direction and direction == old_direction and (dx or dy):
                if player.get('animation') == old.get('animation'):
                    if animation_since is None:
                        animation_since = previous['input_observed_ns']
                    if now-animation_since > 100_000_000:
                        invariant = 'stale_animation'
                else:
                    animation_since = None
            else:
                animation_since = None
            if invariant and len(failures) < 16:
                failures.append(dict(invariant=invariant, frame=frame['frame'], tick=frame['tick'],
                    previous_position=old['position'], position=player['position'], keys=frame['keys']))
            checked += 1
            previous = frame
    return dict(passed=not failures and checked > 0 and (not peer or compared > 0),
                checked_render_frames=checked, matching_tick_hash_samples=compared,
                first_failure=failures[0] if failures else None, failures=failures)
