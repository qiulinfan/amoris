"""Thornkeep's animation graphs (docs/spec/animation.md 3): one per character skeleton, the same
moveset with each character's stance. Writes anim/<character>.anim.json.

    python samples/thornkeep/build_anims.py

Clip times are seconds into the Universal Animation Library's clips (models/anim/ual1.glb and
ual2.glb). Notifies: `hit` windows (the blade can wound), `chain` windows (the next swing may be
queued), `release` (a thrown or cast thing leaves the hand), `evade` (invulnerable while rolling),
`step` (a footfall).
"""
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))

CLIPS = {
    'Sword_Regular_A': {'notifies': [{'name': 'swing', 'at': 0.08}, {'name': 'hit', 'from': 0.12, 'to': 0.30}]},
    'Sword_Regular_A_Rec': {'notifies': [{'name': 'chain', 'from': 0.0, 'to': 0.45}]},
    'Sword_Regular_B': {'notifies': [{'name': 'swing', 'at': 0.10}, {'name': 'hit', 'from': 0.14, 'to': 0.36}]},
    'Sword_Regular_B_Rec': {'notifies': [{'name': 'chain', 'from': 0.0, 'to': 0.50}]},
    'Sword_Regular_C': {'notifies': [{'name': 'swing', 'at': 0.45}, {'name': 'hit', 'from': 0.50, 'to': 0.95}]},
    'Sword_Attack_RM': {'root_motion': True, 'notifies': [{'name': 'swing', 'at': 0.48}, {'name': 'hit', 'from': 0.52, 'to': 0.86}]},
    'Sword_Dash_RM': {'root_motion': True, 'notifies': [{'name': 'swing', 'at': 0.40}, {'name': 'hit', 'from': 0.42, 'to': 0.80}]},
    'Sword_Regular_Combo': {'notifies': [
        {'name': 'swing', 'at': 0.30}, {'name': 'hit', 'from': 0.34, 'to': 0.60},
        {'name': 'swing', 'at': 1.10}, {'name': 'hit', 'from': 1.14, 'to': 1.40},
        {'name': 'swing', 'at': 1.95}, {'name': 'hit', 'from': 2.00, 'to': 2.40}]},
    'Melee_Hook': {'notifies': [{'name': 'swing', 'at': 0.10}, {'name': 'hit', 'from': 0.15, 'to': 0.35}]},
    'Shield_OneShot': {'notifies': [{'name': 'swing', 'at': 0.20}, {'name': 'hit', 'from': 0.25, 'to': 0.45}]},
    'Shield_Dash_RM': {'root_motion': True, 'notifies': [{'name': 'hit', 'from': 0.20, 'to': 0.80}]},
    'Spell_Simple_Shoot': {'notifies': [{'name': 'release', 'at': 0.22}]},
    'OverhandThrow': {'notifies': [{'name': 'release', 'at': 0.62}]},
    'Roll_RM': {'root_motion': True, 'notifies': [{'name': 'evade', 'from': 0.04, 'to': 0.58}]},
    'Hit_Knockback_RM': {'root_motion': True},
    'Walk_Loop': {'notifies': [{'name': 'step', 'at': 0.10}, {'name': 'step', 'at': 0.78}]},
    'Jog_Fwd_Loop': {'notifies': [{'name': 'step', 'at': 0.05}, {'name': 'step', 'at': 0.53}]},
    'Sprint_Loop': {'notifies': [{'name': 'step', 'at': 0.05}, {'name': 'step', 'at': 0.40}]},
}


def montage(slot, sections, fade_in=0.08, fade_out=0.18):
    return {'slot': slot, 'fade_in': fade_in, 'fade_out': fade_out, 'sections': sections}


def sec(name, clip, nxt=None, rate=1.0):
    s = {'name': name, 'clip': clip}
    if nxt:
        s['next'] = nxt
    if rate != 1.0:
        s['rate'] = rate
    return s


MONTAGES = {
    'light_a': montage('full', [sec('a', 'Sword_Regular_A', 'a_rec', 1.1), sec('a_rec', 'Sword_Regular_A_Rec')]),
    'light_b': montage('full', [sec('b', 'Sword_Regular_B', 'b_rec', 1.1), sec('b_rec', 'Sword_Regular_B_Rec')]),
    'light_c': montage('full', [sec('c', 'Sword_Regular_C', None, 1.15)], fade_out=0.25),
    'heavy': montage('full', [sec('heavy', 'Sword_Attack_RM')], fade_in=0.1, fade_out=0.25),
    'dash': montage('full', [sec('dash', 'Sword_Dash_RM')], fade_in=0.06, fade_out=0.25),
    'combo3': montage('full', [sec('combo', 'Sword_Regular_Combo')], fade_out=0.25),
    'hook': montage('full', [sec('hook', 'Melee_Hook', 'rec'), sec('rec', 'Melee_Hook_Rec')]),
    'roll': montage('full', [sec('roll', 'Roll_RM', None, 1.2)], fade_in=0.04, fade_out=0.15),
    'hit_chest': montage('full', [sec('hit', 'Hit_Chest')], fade_in=0.03, fade_out=0.12),
    'hit_head': montage('full', [sec('hit', 'Hit_Head')], fade_in=0.03, fade_out=0.12),
    'knockback': montage('full', [sec('hit', 'Hit_Knockback_RM')], fade_in=0.03, fade_out=0.2),
    'guard_break': montage('full', [sec('break', 'Idle_Shield_Break')], fade_in=0.04, fade_out=0.2),
    'bash': montage('full', [sec('bash', 'Shield_OneShot')], fade_out=0.2),
    'charge': montage('full', [sec('charge', 'Shield_Dash_RM')], fade_in=0.06, fade_out=0.25),
    'cast': montage('full', [sec('cast', 'Spell_Simple_Shoot')], fade_in=0.06, fade_out=0.15),
    'throw': montage('full', [sec('throw', 'OverhandThrow', None, 1.15)], fade_out=0.2),
    'drink': montage('upper', [sec('drink', 'Consume')], fade_in=0.15, fade_out=0.2),
}

# Per character: its skeleton file, its idle and its guard.
CAST = {
    'sellsword': {'idle': 'Sword_Idle', 'guard': 'Sword_Block'},
    'brigand': {'idle': 'Sword_Idle', 'guard': 'Sword_Block'},
    'brute': {'idle': 'Idle_Shield_Loop', 'guard': 'Idle_Shield_Loop'},
    'archer': {'idle': 'Idle_Loop', 'guard': 'Sword_Block'},
    'warden': {'idle': 'Idle_Shield_Loop', 'guard': 'Idle_Shield_Loop'},
}


def graph(name, c):
    return {
        'format': 'amoris-animgraph',
        'version': 1,
        'skeleton': f'models/chars/{name}.glb',
        'libraries': ['models/anim/ual1.glb', 'models/anim/ual2.glb'],
        'sockets': ['hand_r', 'hand_l', 'Head', 'spine_03', 'pelvis'],
        'params': {'speed': 0, 'blocking': False, 'dead': False},
        'clips': CLIPS,
        'masks': {'upper': [{'joint': 'spine_01', 'weight': 0.6}, {'joint': 'spine_02'}]},
        'layers': [
            {'name': 'base', 'start': 'move', 'states': {
                'move': {'blend1d': {'param': 'speed', 'points': [
                    [0, c['idle']], [1.6, 'Walk_Loop'], [3.8, 'Jog_Fwd_Loop'], [6.2, 'Sprint_Loop']]}},
                'block': {'clip': c['guard'], 'loop': c['guard'].endswith('_Loop'), 'rate': 1.6},
                'dead': {'clip': 'Death01', 'loop': False},
            }, 'transitions': [
                {'from': '*', 'to': 'dead', 'when': [['dead', '==', True]], 'fade': 0.15},
                {'from': 'move', 'to': 'block', 'when': [['blocking', '==', True]], 'fade': 0.12},
                {'from': 'block', 'to': 'move', 'when': [['blocking', '==', False]], 'fade': 0.2},
            ]},
            {'name': 'upper', 'mask': 'upper', 'slot': 'upper'},
            {'name': 'full', 'slot': 'full'},
        ],
        'montages': MONTAGES,
    }


def main():
    os.makedirs(os.path.join(HERE, 'anim'), exist_ok=True)
    for name, c in CAST.items():
        path = os.path.join(HERE, 'anim', f'{name}.anim.json')
        with open(path, 'w', encoding='utf-8', newline='\n') as f:
            json.dump(graph(name, c), f, indent=1)
            f.write('\n')
        print(path)


if __name__ == '__main__':
    main()
