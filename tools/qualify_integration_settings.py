#!/usr/bin/env python3
"""Owned manifest-specific Direct/settings qualification, without a live backend."""
from __future__ import annotations

import argparse
from contextlib import closing
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import stat
import subprocess
import tempfile

from qualify_supervised_delegation import Fixture, terminate_qualification

ROOT = Path(__file__).resolve().parents[1]
INSPECTIONS = {'copies':0,'file_bytes':0,'removed':0}


def sha(data):
    return hashlib.sha256(data).hexdigest()


def encoded(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def response_domain(value):
    # Rendering correlation is distinct from the retained canonical success.
    return {key: child for key, child in value.items() if key not in ('request_id', 'generated_at')}


def native_tree(root):
    result = []
    for path in [root, *sorted(root.rglob('*'))]:
        info = path.lstat()
        result.append({'path':str(path.relative_to(root)), 'dev':info.st_dev, 'ino':info.st_ino,
                       'mode':info.st_mode, 'size':info.st_size, 'mtime_ns':info.st_mtime_ns,
                       'ctime_ns':info.st_ctime_ns,
                       'sha256':sha(path.read_bytes()) if stat.S_ISREG(info.st_mode) else None,
                       'target':os.readlink(path) if stat.S_ISLNK(info.st_mode) else None})
    return result


def stored(worktree):
    # Completed owned CLI children are quiescent. Inspect an exact main+WAL
    # capture read-only; SQLite may create SHM only beside this disposable copy.
    database = worktree / '.podway/runtime/state.sqlite3'
    sources = [database]
    wal = database.with_name(database.name + '-wal')
    if wal.exists():
        sources.append(wal)
    before = {'runtime':native_tree(worktree / '.podway/runtime'), 'git':native_tree(worktree / '.git')}
    with tempfile.TemporaryDirectory(prefix='podway-integ005-inspection-') as directory:
        image = Path(directory) / database.name
        for source in sources:
            data = source.read_bytes()
            (image.parent / source.name).write_bytes(data)
            INSPECTIONS['file_bytes'] += len(data)
        INSPECTIONS['copies'] += 1
        with closing(sqlite3.connect(image.as_uri() + '?mode=ro', uri=True)) as connection:
            tables = {}
            for (name,) in connection.execute("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name"):
                quoted = '"' + name.replace('"', '""') + '"'
                rows = connection.execute('SELECT * FROM ' + quoted).fetchall()
                tables[name] = sorted([list(row) for row in rows], key=encoded)
            outcomes = {row[0]:{'request_digest':row[1], 'command':row[2], 'sequence':row[3],
                                 'response_json':row[6], 'response_sha256':sha(row[6].encode())}
                        for row in tables['mutation_outcomes']}
            result = {'tables':tables, 'sequence':tables['workspace_state'][0][5],
                      'writer':tables['writer_contract'], 'snapshots':tables['v2_procedure_snapshots'],
                      'outcomes':outcomes, 'schema_version':connection.execute('PRAGMA user_version').fetchone()[0]}
    assert not Path(directory).exists()
    INSPECTIONS['removed'] += 1
    assert {'runtime':native_tree(worktree / '.podway/runtime'), 'git':native_tree(worktree / '.git')} == before, 'inspection changed authentic runtime/Git'
    return result


class SettingsFixture(Fixture):
    def __init__(self, cli, root, records):
        super().__init__(cli, root)
        self.records, self.home = records, None
        self.sentinel = root / 'sentinel-bin'
        self.sentinel.mkdir(exist_ok=True)
        self.marker = root / 'backend-was-launched'
        backend = self.sentinel / 'dolgorae'
        if not backend.exists():
            backend.write_text('#!/bin/sh\ntouch "' + str(self.marker) + '"\nexit 91\n')
            backend.chmod(0o700)

    def call(self, worktree, args, *, payload=None, payload_bytes=None, error=None):
        assert worktree.is_relative_to(self.root) and worktree.is_dir()
        assert payload is None or payload_bytes is None
        body = json.dumps(payload).encode() if payload is not None else payload_bytes
        workspace = [] if args[0] in {'help','version','completions','procedure','preset'} else ['--worktree',str(worktree)]
        argv = [str(self.cli),'--json',*workspace,*args]
        env = {'PATH':str(self.sentinel) + ':/usr/bin:/bin'}
        if self.home is not None:
            env['HOME'] = str(self.home)
        child = subprocess.Popen(argv, cwd=worktree, env=env,
                                 stdin=subprocess.PIPE if body is not None else subprocess.DEVNULL,
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
        try:
            stdout, stderr = child.communicate(body, timeout=30)
            assert len(stdout) <= 1048578 and len(stderr) <= 65536, 'bounded native CLI output'
        finally:
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGKILL)
            child.wait(timeout=5)
            for stream in (child.stdin, child.stdout, child.stderr):
                if stream is not None:
                    stream.close()
        record = {'argv':argv, 'request_args':list(args), 'stdin_hex':None if body is None else body.hex(),
                  'home':None if self.home is None else str(self.home), 'exit_code':child.returncode,
                  'stdout':stdout.decode(), 'stderr':stderr.decode()}
        self.records.append(record)
        assert not stderr, record
        value = json.loads(stdout)
        if error:
            assert child.returncode == value['exit_code'] != 0 and value['schema']=='podway.error/v2', record
            assert value['code'] == error, record
        else:
            assert child.returncode == 0 and value['schema']=='podway.output/v4', record
        assert not self.marker.exists(), 'ordinary Direct/query launched backend sentinel'
        return value


def toggle(worktree, enabled):
    (worktree/'.podway/config.yaml').write_text('schema: podway.workspace/v3\nintegrations: {dolgorae: {enabled: ' + str(enabled).lower() + '}}\n')


def accepted_fixture(fixture, name):
    worktree = fixture.workspace(name)
    payload, _ = fixture.acceptance(worktree)
    original = fixture.accept(worktree, payload)
    request = json.loads(json.dumps(fixture.records[-1]))
    receipt = fixture.receipt(worktree, payload)
    state = stored(worktree)
    retained = state['outcomes'][payload['idempotency_key']]
    assert response_domain(json.loads(retained['response_json'])) == response_domain(original)
    return worktree, payload, original, request, receipt, state


def replay(fixture, worktree, request, error=None):
    return fixture.call(worktree, request['request_args'], payload_bytes=bytes.fromhex(request['stdin_hex']), error=error)


def recover(fixture, worktree, key, canonical):
    lookup = fixture.call(worktree, ['outcome','lookup','--idempotency-key',key])['result']
    assert lookup['found'] is True and lookup['outcome']['response'] == canonical
    return lookup


def current_matrix(fixture, poison):
    worktree, payload, original, request, receipt, initial = accepted_fixture(fixture, 'current')
    canonical = json.loads(initial['outcomes'][payload['idempotency_key']]['response_json'])
    result = {'matrix_cases':0, 'original_request':request, 'original_response':original,
              'original_retained_success':initial['outcomes'][payload['idempotency_key']],
              'original_receipt':receipt, 'initial_state':initial, 'cases':[]}
    for enabled in [True,False]:
        for home in [None,poison]:
            toggle(worktree,enabled);fixture.home=home
            before = stored(worktree)
            status = fixture.call(worktree,['status'])
            assert status['result']['session']['id'] == payload['session_id']
            assert fixture.receipt(worktree,payload)['receipt'] == receipt['receipt']
            recover(fixture,worktree,payload['idempotency_key'],canonical)
            assert stored(worktree) == before, 'ordinary reads changed state'
            fixture.mutate(worktree,['block','--reason','Owned continuity check'])
            assert stored(worktree)['sequence'] == before['sequence'] + 1
            fixture.mutate(worktree,['unblock','--all'])
            after_write = stored(worktree)
            assert after_write['sequence'] == before['sequence'] + 2
            assert after_write['writer'] == initial['writer'] and after_write['snapshots'] == initial['snapshots']
            assert response_domain(replay(fixture,worktree,request)) == response_domain(original)
            recover(fixture,worktree,payload['idempotency_key'],canonical)
            assert fixture.receipt(worktree,payload)['receipt'] == receipt['receipt']
            assert stored(worktree) == after_write, 'original replay/lookup changed persisted state'
            result['cases'].append({'enabled':enabled,'home':'absent' if home is None else 'poisoned',
                                    'sequence_before':before['sequence'],'sequence_after_fresh_writes':after_write['sequence'],
                                    'logical_before_replay_sha256':sha(encoded(after_write)),
                                    'logical_after_replay_sha256':sha(encoded(stored(worktree))),
                                    'retained_original_response_sha256':after_write['outcomes'][payload['idempotency_key']]['response_sha256']})
            result['matrix_cases'] += 1
    toggle(worktree,False);fixture.home=poison
    before = stored(worktree)
    completed = fixture.mutate(worktree,['complete'])
    assert completed['result']['session_state'] == 'completed'
    final = stored(worktree)
    assert final['sequence'] == before['sequence'] + 1
    assert fixture.receipt(worktree,payload)['applicability'] == 'historical'
    assert fixture.receipt(worktree,payload)['receipt'] == receipt['receipt']
    assert response_domain(replay(fixture,worktree,request)) == response_domain(original)
    recover(fixture,worktree,payload['idempotency_key'],canonical)
    assert stored(worktree) == final
    assert final['writer'] == initial['writer'] and final['snapshots'] == initial['snapshots']
    result.update(final_state=final, explicit_disabled_completion=completed, passed=True)
    return result


def unchanged_call(fixture, worktree, args, *, payload_bytes=None, error=None):
    before = native_tree(worktree)
    value = fixture.call(worktree,args,payload_bytes=payload_bytes,error=error)
    after = native_tree(worktree)
    assert before == after, 'candidate changed previous Store/Git/runtime bytes or native witnesses'
    return value, {'argv':fixture.records[-1]['argv'], 'before':before, 'after':after, 'unchanged':True}


def previous_matrix(creator, candidate, poison, valid_home, previous_manifest, candidate_manifest):
    worktree, payload, original, request, receipt, initial = accepted_fixture(creator,'previous')
    assert initial['writer'][0][1] == previous_manifest, initial['writer']
    fences = creator.fences(creator.observe(worktree))
    result={'authentic_creator':'selected previous CLI; no SQLite writer fabrication', 'initial_state':initial,
            'original_request':request,'original_response':original,'original_receipt':receipt,'refusals':[], 'queries':[]}
    for enabled in [True,False]:
        for home in [None,poison]:
            toggle(worktree,enabled);candidate.home=home
            operations=[(['status'],None),
                        (['outcome','lookup','--idempotency-key',payload['idempotency_key']],None),
                        (['delegation','receipt','--session',payload['session_id'],'--attempt',payload['attempt_id']],None),
                        (['block','--reason','Must refuse',*fences,'--idempotency-key','candidate-must-refuse'],None),
                        (request['request_args'],bytes.fromhex(request['stdin_hex']))]
            for args, body in operations:
                error,witness=unchanged_call(candidate,worktree,args,payload_bytes=body,error='WORKSPACE_CONTRACT_MISMATCH')
                assert error['details']['actual_contract_manifest_digest']==previous_manifest
                assert error['details']['expected_contract_manifest_digest']==candidate_manifest
                result['refusals'].append({'enabled':enabled,'home':'absent' if home is None else 'poisoned',
                                           'error':error,'witness':witness})
    for generation in ['1','2']:
        args=['integration','show','dolgorae','--contract-generation',generation]
        for enabled in [False,True]:
            toggle(worktree,enabled);candidate.home=valid_home
            value,witness=unchanged_call(candidate,worktree,args)
            assert value['result']=={'schema':'podway.integration-settings-result/v1','integration':'dolgorae','enabled':enabled,'source':'project'}
            assert 'workspace' not in value and 'session' not in value
            result['queries'].append({'generation':generation,'result':value,'witness':witness})
            for home in [None,poison,worktree/'missing-home']:
                candidate.home=home
                error,witness=unchanged_call(candidate,worktree,args,error='USER_CONFIG_INVALID')
                assert error['details']=={} and error['retryable'] is False
                assert str(candidate.root) not in json.dumps(error) and 'reset' not in json.dumps(error).lower()
                result['queries'].append({'generation':generation,'error':error,'witness':witness})
        (worktree/'.podway/config.yaml').write_text('invalid project')
        for invalid_home in [poison,None,worktree/'missing-home']:
            candidate.home=invalid_home
            error,witness=unchanged_call(candidate,worktree,args,error='WORKSPACE_CONFIG_INVALID')
            assert error['details']=={} and 'reset' not in json.dumps(error).lower() and str(candidate.root) not in json.dumps(error)
            result['queries'].append({'generation':generation,'error':error,'witness':witness})
    toggle(worktree,True);candidate.home=valid_home
    runtime=worktree/'.podway/runtime';held=worktree/'.podway/runtime-held'
    runtime.rename(held);runtime.write_text('owned blocked runtime')
    try:
        for generation in ['1','2']:
            value,witness=unchanged_call(candidate,worktree,['integration','show','dolgorae','--contract-generation',generation])
            assert value['result']['enabled'] is True
            result['queries'].append({'blocked_runtime':True,'generation':generation,'result':value,'witness':witness})
    finally:
        runtime.unlink();held.rename(runtime)
    assert stored(worktree)==initial
    result.update(refusal_cases=len(result['refusals']),query_cases=len(result['queries']), final_state=stored(worktree),passed=True)
    return result


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--podway',required=True,type=Path)
    parser.add_argument('--previous',type=Path)
    parser.add_argument('--previous-sha256')
    parser.add_argument('--previous-identity',type=Path)
    parser.add_argument('--output',type=Path)
    args=parser.parse_args()
    if not __debug__:
        raise RuntimeError('qualification assertions must be enabled')
    assert bool(args.previous)==bool(args.previous_sha256)==bool(args.previous_identity)
    signal.signal(signal.SIGTERM,terminate_qualification)
    binaries={'candidate':args.podway}
    if args.previous:binaries['previous']=args.previous
    pins={}
    for name,path in binaries.items():
        assert path.is_absolute() and path.resolve()==path and path.is_file() and not path.is_symlink()
        pins[name]={'path':str(path),'sha256':sha(path.read_bytes())}
    if args.previous:assert pins['previous']['sha256']==args.previous_sha256
    report={'schema':'podway.integration-settings-qualification/v1','activation_qualified':False,
            'binaries':pins,'records':[],'limits':['Owned local persistence/receipt qualification with deterministic external fixture, not a real backend/task/model proof.',
                'No kernel file-access tracing: Store-free query is established by source separation, blocked-runtime controls and byte/native-witness preservation.',
                'Selected previous binary identity is pinned local evidence, not a new published-release provenance claim.',
                'Outer rendering request/time fields are excluded from response-domain comparison; retained canonical success bytes and lookup response identity are checked independently.']}
    with tempfile.TemporaryDirectory(prefix='podway-integ005-') as directory:
        root=Path(directory).resolve();report['fixture_root']=str(root)
        candidate=SettingsFixture(args.podway,root,report['records'])
        poison=root/'poisoned-home';(poison/'.config/podway').mkdir(parents=True)
        (poison/'.config/podway/config.yaml').write_text('invalid user configuration')
        valid=root/'valid-home';(valid/'.config/podway').mkdir(parents=True)
        (valid/'.config/podway/config.yaml').write_text('schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: true}}\n')
        pins['candidate']['identity']=candidate.call(root,['version','--identity'])
        expected=json.loads((ROOT/'contracts/contract-manifest-v2.json').read_text())['digest']
        assert pins['candidate']['identity']['result']['contract_manifest_digest']==expected
        report['current']=current_matrix(candidate,poison)
        assert report['current']['initial_state']['writer'][0][1]==expected
        if args.previous:
            creator=SettingsFixture(args.previous,root,report['records'])
            pins['previous']['identity']=creator.call(root,['version','--identity'])
            selected=json.loads(args.previous_identity.read_text())
            assert pins['previous']['identity']['result']==selected['identity']['result']
            assert selected['sha256']==pins['previous']['sha256']
            report['previous_identity_input']={'path':str(args.previous_identity),'sha256':sha(args.previous_identity.read_bytes())}
            previous_manifest=pins['previous']['identity']['result']['contract_manifest_digest']
            assert previous_manifest!=expected
            report['previous']=previous_matrix(creator,candidate,poison,valid,previous_manifest,expected)
        report['fixture_file_bytes_before_cleanup']=sum(p.stat().st_size for p in root.rglob('*') if p.is_file() and not p.is_symlink())
        assert not candidate.marker.exists()
        for name,path in binaries.items():assert sha(path.read_bytes())==pins[name]['sha256']
    report['cleanup']={'fixture_removed':not root.exists(),'owned_root':str(root),'bounded_children_reaped':True,'inspection_copies':INSPECTIONS}
    assert INSPECTIONS['copies']==INSPECTIONS['removed']
    report['logical_inspection']='Quiescent byte-identical main and present WAL copies inspected via read-only SQLite in separate disposable scratch; absent WAL remains absent in the capture. Original runtime/Git native bytes/witnesses checked unchanged on every capture. This observes quiescent owned fixtures, not an atomic snapshot against concurrent external writers. No authentic Store checkpoint or write; inspection-copy SHM removed with each copy.'
    if args.output:
        with args.output.open('x') as stream:json.dump(report,stream,indent=2);stream.write('\n')
    print(json.dumps({'schema':report['schema'],'current':{'matrix_cases':report['current']['matrix_cases'],'passed':report['current']['passed']},
                      'previous':None if 'previous' not in report else {'passed':report['previous']['passed'],'refusal_cases':report['previous']['refusal_cases'],'query_cases':report['previous']['query_cases']},
                      'cleanup':report['cleanup'],'activation_qualified':False,'report':str(args.output) if args.output else None}))


if __name__=='__main__':main()
