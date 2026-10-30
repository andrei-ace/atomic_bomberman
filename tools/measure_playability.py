"""Measure OS-injected keys against in-memory traces from actual game frames."""
import copy
import json
import math
import statistics
import time


def summary(values):
    values=sorted(v for v in values if v is not None)
    if not values:return dict(samples=0,median=None,p95=None,p99=None,max=None)
    return dict(samples=len(values),median=statistics.median(values),
                p95=values[math.ceil(.95*len(values))-1],p99=values[math.ceil(.99*len(values))-1],max=values[-1])


def run(host,client,focus,key,press,snapshot,lobby_action,repetitions=10,single_play=False):
    state=snapshot(host,'Battle' if single_play else 'NetworkLobby')
    config=copy.deepcopy(state['simulation']['config'] if single_play else state['network']['lobby']['config'])
    config.update(countdown_ms=0,random_starts=False,enclosement_depth=0)
    config['scheme'].update(brick_density=0,name='Controlled playability arena',
                           rows=['###############']+['#.............#']*9+['###############'])
    if single_play:
        for player in config['players']:player['control']='Human'
    coordinates=[(2,2),(8,6),(12,2),(2,8),(6,2),(8,2),(10,4),(12,8),(4,6),(6,8)]
    for start,(x,y) in zip(config['scheme']['starts'],coordinates):start['tile']=dict(x=x,y=y)
    for power in config['scheme']['powerups']:
        power['born_with']={'Bomb':3,'Flame':2,'Trigger':1}.get(power['kind'],0)
        power['override_count']=0
    (host[1]/'test-config.json').write_text(json.dumps(config))
    focus(host[0]);press('F9')
    if not single_play:
        assert snapshot(host,'NetworkLobby')['network']['lobby']['config']['countdown_ms']==0
        lobby_action(host,'Start')

    def playing():
        deadline=time.monotonic()+30
        while time.monotonic()<deadline:
            if single_play:
                state=snapshot(client)
                if state['screen']=='Battle' and state['simulation']['phase']=='Playing':return [state,state]
            else:
                states=[snapshot(host),snapshot(client)]
                if all(s['screen']=='Battle' and not s['network']['starting'] and s['network']['confirmed_simulation']['phase']=='Playing' for s in states):return states
            time.sleep(.1)
        raise AssertionError('Both game windows did not enter play')

    _,state=playing()
    seat=0 if single_play else state['network']['lobby']['owners'].index(state['network']['id'])
    commands=[];focus(client[0]);time.sleep(.4)
    for trial in range(repetitions):
        issued=time.time_ns();key('Right');time.sleep(.12)
        turn=time.time_ns();key('Right',False);key('Left');time.sleep(.12)
        released=time.time_ns();key('Left',False);time.sleep(.5)
        commands.append(dict(kind='movement',trial=trial,key='Right',issued_ns=issued,turn_ns=turn,release_ns=released,until_ns=time.time_ns()))
    for trial in range(repetitions):
        focus(host[0]);press('F10');playing();focus(client[0]);time.sleep(.2)
        issued=time.time_ns();key('space');time.sleep(.08);key('space',False);time.sleep(.35)
        key('Right');time.sleep(.7);key('Right',False);time.sleep(.25)
        action=time.time_ns();key('Control_R');time.sleep(.08);key('Control_R',False);time.sleep(.65)
        commands.append(dict(kind='bomb_action',trial=trial,key='Space',issued_ns=issued,action_ns=action,until_ns=time.time_ns()))
    local=snapshot(client,'Battle')['input_trace'];remote=[] if single_play else snapshot(host,'Battle')['input_trace']
    assert local and (single_play or remote),'Opt-in frame traces unavailable'

    def measure(row,name,frames,start,end,predicate):
        frame=next((f for f in frames if f.get('swap_returned_ns') is not None
                    and start<=f['input_observed_ns'] and f['swap_returned_ns']<end and predicate(f)),None)
        row[name]=None if frame is None else (frame['swap_returned_ns']-start)/1e6
        row[name.replace('_ms','_draw_ready_ms')]=None if frame is None else (frame['draw_submitted_ns']-start)/1e6
    def before(frames,timestamp):return next((f for f in reversed(frames) if f['draw_submitted_ns']<timestamp),None)
    def p(frame,canonical=False):return frame['confirmed_players' if canonical else 'players'][seat]['position']
    rows=[]
    for command in commands:
        start,end=command['issued_ns'],command['until_ns'];initial=before(local,start)
        assert initial is not None,'Trace ring lost a trial'
        row=dict(command)
        row['os_key_to_app_observed_ms']=next(((f['input_observed_ns']-start)/1e6 for f in local
            if start<=f['input_observed_ns']<end and command['key'] in f['keys']),None)
        if command['kind']=='movement':
            x0=p(initial)['x'];c0=p(initial,True)['x'];r0=0 if single_play else p(before(remote,start))['x']
            turn,release=command['turn_ns'],command['release_ns']
            measure(row,'key_to_local_frame_ms',local,start,turn,lambda f:p(f)['x']>=x0+100 and f['players'][seat]['facing']=='Right')
            measure(row,'key_to_confirmed_state_ms',local,start,end,lambda f:p(f,True)['x']>=c0+100)
            measure(row,'key_to_peer_frame_ms',remote,start,end,lambda f:p(f)['x']>=r0+100)
            previous=before(local,turn)
            for frame in local:
                if not turn<=frame['input_observed_ns']<release or not frame.get('swap_returned_ns'):continue
                if p(frame)['x']<p(previous)['x'] and frame['players'][seat]['facing']=='Left':
                    row['reversal_to_local_frame_ms']=(frame['swap_returned_ns']-turn)/1e6;row['reversal_to_local_frame_draw_ready_ms']=(frame['draw_submitted_ns']-turn)/1e6;break
                previous=frame
            previous=before(local,release);last=release;distance=0.;backward=0.
            for frame in local:
                if not release<=frame['input_observed_ns']<end or frame.get('swap_returned_ns') is None:continue
                delta=p(frame)['x']-p(previous)['x']
                if delta:
                    last=frame['swap_returned_ns'];distance+=abs(delta)/100.
                    if delta>0:backward+=delta/100.
                previous=frame
            row.update(release_to_last_motion_ms=max(0,(last-release)/1e6),post_release_motion_pixels=distance,backward_after_release_pixels=backward)
        else:
            def placed(frame,canonical=False):return any(b['owner']==seat for b in frame['confirmed_bombs' if canonical else 'bombs'])
            action=command['action_ns']
            measure(row,'key_to_local_frame_ms',local,start,action,placed)
            measure(row,'key_to_confirmed_state_ms',local,start,action,lambda f:placed(f,True))
            measure(row,'key_to_peer_frame_ms',remote,start,action,placed)
            measure(row,'action_to_local_flame_ms',local,action,end,lambda f:bool(f['flames']))
            measure(row,'action_to_confirmed_flame_ms',local,action,end,lambda f:bool(f['confirmed_flames']))
            measure(row,'action_to_peer_flame_ms',remote,action,end,lambda f:bool(f['flames']))
            row['owner_alive_after_dodge']=all(f['confirmed_players'][seat]['alive'] for f in local if action<=f['input_observed_ns']<end)
        rows.append(row)
    movement=[r for r in rows if r['kind']=='movement'];bombs=[r for r in rows if r['kind']=='bomb_action'];metrics={}
    for name,group in [('movement',movement),('bomb',bombs)]:
        metrics[name]={field:summary(row.get(field) for row in group) for field in
            ('os_key_to_app_observed_ms','key_to_local_frame_ms','key_to_local_frame_draw_ready_ms','key_to_confirmed_state_ms','key_to_peer_frame_ms')}
    metrics['movement'].update({field:summary(row.get(field) for row in movement) for field in
        ('reversal_to_local_frame_ms','reversal_to_local_frame_draw_ready_ms','release_to_last_motion_ms','post_release_motion_pixels','backward_after_release_pixels')})
    metrics['action']={field:summary(row.get(field) for row in bombs) for field in
        ('action_to_local_flame_ms','action_to_local_flame_draw_ready_ms','action_to_confirmed_flame_ms','action_to_peer_flame_ms')}
    required=('key_to_local_frame_ms','key_to_confirmed_state_ms')+(() if single_play else ('key_to_peer_frame_ms',))
    action_required=('action_to_local_flame_ms','action_to_local_flame_draw_ready_ms','action_to_confirmed_flame_ms')+(() if single_play else ('action_to_peer_flame_ms',))
    missed=sum(any(r.get(k) is None for k in required) for r in rows)
    missed+=sum(any(r.get(k) is None for k in action_required) for r in bombs)
    from render_gates import inspect
    correctness = inspect(local, remote, commands, seat)
    return dict(passed=missed==0 and all(r['owner_alive_after_dodge'] for r in bombs) and correctness['passed'],correctness=correctness,version=local[0]['version'],protocol=local[0]['protocol'],
        mode='single-play' if single_play else 'network',
        environment='Actual offline game window; XTest OS key events; Xvfb and Mesa' if single_play else 'Actual game windows; XTest OS key events; Xvfb and Mesa; real Internet relay',
        measurement='OS key issuance to app observation, draw submission and next-frame resumption bounds, canonical effect and peer frame',
        _raw_traces=dict(local=local,peer=remote),metrics=metrics,missed_actions=missed,late_dodge_rejections=sum(not r['owner_alive_after_dodge'] for r in bombs),
        observed_rtt_ms=summary(f['rtt_ms'] for f in local if f['rtt_ms'] is not None),trials=rows,
        trace_frames=dict(local=len(local),peer=len(remote)),limitations=[
            'Synthetic OS keys, not a physical keyboard/controller.',
            'Draw submission and next-frame resumption bracket software presentation; physical display scan-out and speakers are not measured.',
            'Offline comparison uses one controlled avatar and a stationary opponent in the same arena.' if single_play else 'Both rendered players use one test machine and connect outward through the real relay.',
            'A controlled open arena isolates effects; competitive matches also need human assessment.'])
