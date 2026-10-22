#!/usr/bin/env python3
"""Rewrite a COPY of library.json with a realistic demo inventory.
usage: mkdata.py <data-dir>   (the aurora-lab host keeps its id/credentials)
All demo hosts point at 127.0.0.N:2222 (the local test sshd) so they show as reachable."""
import json, sys, uuid, copy

p = sys.argv[1] + '/library.json'
d = json.load(open(p))
lab = next(h for h in d['hosts'] if h['label'] == 'aurora-lab')
tmpl = copy.deepcopy(d['hosts'][0])


def gid():
    return str(uuid.uuid4())


groups = []


def add(name, parent=None, color=None, env='None'):
    g = {"id": gid(), "name": name, "parent_id": parent, "color": color, "icon": "",
         "default_username": "", "default_identity": None, "env": env, "collapsed": False}
    groups.append(g)
    return g['id']


hosts = []
n = [10]


def host(label, group, tags=(), env='None', user='ops', fav=False, proto='Ssh'):
    h = copy.deepcopy(tmpl)
    h.update(id=gid(), label=label, group_id=group, hostname='127.0.0.%d' % n[0], port=2222,
             username=user, tags=list(tags), env=env, favorite=fav, notes='',
             last_connected=None, jump_host=None, startup_snippet=None, protocol=proto)
    n[0] += 1
    hosts.append(h)


acme = add('Acme GmbH', None, [94, 234, 212])
acme_dc = add('Frankfurt DC', acme, [248, 113, 113], 'Prod')
acme_of = add('Berlin Office', acme, [94, 234, 212])
nord = add('Nordwind AG', None, [129, 161, 193])
nord_h = add('Hamburg', nord, [248, 113, 113], 'Prod')
nord_m = add('Munich', nord, [129, 161, 193], 'Staging')
glob = add('Globex Ltd', None, [245, 166, 35])
glob_l = add('London', glob, [245, 166, 35])
home = add('Homelab', None, [167, 139, 250])
host('web-01', acme_dc, ['nginx', 'web'], 'Prod', 'deploy', True)
host('web-02', acme_dc, ['nginx', 'web'], 'Prod', 'deploy')
host('db-primary', acme_dc, ['postgres', 'db'], 'Prod', 'postgres')
host('db-replica', acme_dc, ['postgres', 'db'], 'Prod', 'postgres')
host('fw-berlin', acme_of, ['firewall'], 'None', 'admin')
host('print-srv', acme_of, ['cups'], 'None', 'admin')
host('app-hh-01', nord_h, ['java', 'app'], 'Prod', 'svc')
host('app-hh-02', nord_h, ['java', 'app'], 'Prod', 'svc')
host('mon-hh', nord_h, ['prometheus', 'monitoring'], 'Prod', 'ops')
host('stage-app', nord_m, ['java', 'app'], 'Staging', 'svc')
host('stage-db', nord_m, ['postgres', 'db'], 'Staging', 'postgres')
host('bastion-ldn', glob_l, ['jump'], 'None', 'ops', True)
host('k8s-node-1', glob_l, ['k8s'], 'None', 'ops')
host('k8s-node-2', glob_l, ['k8s'], 'None', 'ops')
host('gitea', home, ['git', 'dev'], 'None', 'tim')
host('nas', home, ['backup'], 'None', 'tim')
host('win-dc-01', acme_dc, ['windows', 'ad'], 'Prod', 'admin', False, 'Rdp')
host('win-ts-01', nord_h, ['windows', 'terminal'], 'Prod', 'admin', False, 'Rdp')
host('sw-core-01', acme_of, ['switch'], 'None', 'admin', False, 'Telnet')
host('kiosk-01', glob_l, ['kiosk'], 'None', 'pi', False, 'Vnc')
lab.update(notes='Backup runs at 03:00 - do not restart in that window. Contact: ops team.', group_id=home, tags=['test', 'lab'], env='None', username='aurora', favorite=True)
hosts.append(lab)
for h in hosts:
    h['username'] = 'aurora'
    h['identity_id'] = lab['identity_id']   # every demo host logs in with the aurora-lab keychain entry
d['hosts'] = hosts
fp = d['known_hosts']['127.0.0.1:2222']          # all demo hosts are the same sshd -> same host key
for h in hosts:
    d['known_hosts']['%s:%d' % (h['hostname'], h['port'])] = fp
d['groups'] = groups
for k in ('facts', 'pve_servers', 'pbs_servers', 'net', 'runbooks', 'maintenance', 'host_key_history'):
    if isinstance(d.get(k), list):
        d[k] = []
    elif isinstance(d.get(k), dict):
        d[k] = {}
for sn in d['snippets']:
    sn['name'] = {'System-Update': 'System update', 'Logs folgen': 'Follow logs', 'Docker aufräumen': 'Docker cleanup'}.get(sn['name'], sn['name'])
    sn['description'] = {'Pakete aktualisieren': 'Update packages', 'Belegung pro Mount': 'Usage per mount',
                         'journalctl für einen Dienst': 'journalctl for one service',
                         'Ungenutzte Images & Volumes': 'Unused images & volumes'}.get(sn['description'], sn['description'])
for lc in d.get('log_commands', []):
    lc['name'] = 'Service log'
    lc['command'] = 'journalctl -u {{service}} -n {{lines=50}} --no-pager'
d['log_var_history'] = {'lines': ['50'], 'service': ['ssh']}
s = d['settings']
s['language'] = 'en'
s['theme'] = sys.argv[2] if len(sys.argv) > 2 else 'nord'
s['snippet_hints'] = False        # no "save this command as snippet?" toast in the recordings
s['restore_sessions'] = False
s['last_session'] = []
s['auto_lock_minutes'] = 0
s['last_seen_version'] = '0.9.0'
json.dump(d, open(p, 'w'))
print(len(hosts), 'hosts')
