#!/usr/bin/env python3
"""Owned deterministic controls for the repository-only metadata verifier."""
from __future__ import annotations
import copy
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import shutil
import time
from unittest import mock
import unittest

ROOT = Path(__file__).resolve().parents[1]
TOOL = ROOT / 'tools/verify_dolgorae_connection.py'
BUNDLE = ROOT / 'contracts/dolgorae-metadata-v1'

class ConnectionTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='podway-integ004-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.counter = 0
        self.caps = json.loads((BUNDLE/'workspace-capability-v2.valid.json').read_text())
        self.version = {'name':'dolgorae','version':'v'+self.caps['dolgorae_version']}
        self.machine = {'schema_version':4,'ok':True,'command':'capabilities','invocation_id':'00000000-0000-7000-8000-000000000001','data':self.caps}
        self.identity = json.loads((ROOT/'tests/fixtures/v2/protocol/current-contracts.json').read_text())['fixtures']['podway.version-result/v2']
        self.podway = {'schema':'podway.output/v4','request_id':'00000000-0000-4000-8000-000000000001','command':'version','generated_at':'2026-11-01T00:00:00.000Z','result':self.identity,'warnings':[]}

    def executable(self, name, program):
        path=self.root/name;path.write_text('#!'+sys.executable+'\n'+program+'\n');path.chmod(0o700);return path

    def fixtures(self, version=None, capabilities=None, podway=None):
        self.selected_podway=self.executable('podway', 'import json,sys\nassert sys.argv[1:]==["--json","version","--identity"]\nprint('+repr(json.dumps(self.podway if podway is None else podway))+')')
        self.selected_backend=self.executable('dolgorae','import sys\nif sys.argv[1:]==["version","--json"]: print('+repr(json.dumps(self.version if version is None else version))+')\nelif sys.argv[1:]==["runtime","capabilities"]: print('+repr(json.dumps(self.machine if capabilities is None else capabilities))+')\nelse: raise SystemExit(91)')

    def invoke(self, expected=0, extra=()):
        self.counter += 1
        output=self.root/f'report-{self.counter}.json'
        process=subprocess.run([sys.executable,str(TOOL),'--podway',str(self.selected_podway),'--dolgorae',str(self.selected_backend),'--output',str(output),*extra],capture_output=True,timeout=8)
        self.assertEqual(process.returncode,expected,process.stderr.decode()+(output.read_text() if output.exists() else ''))
        report=json.loads(output.read_text());self.assertIs(report['activation_qualified'],False);return report

    def test_stateless_version_and_exact_suffix_success(self):
        self.fixtures();report=self.invoke();self.assertEqual(report['probe_outcome'],'pass')
        self.assertEqual(len(report['probes']),3)
        self.assertEqual(report['bounds']['timeout_seconds'],15)
        self.assertEqual(report['bounds']['stdout_bytes'],1048576)

    def backend_program(self, version_program, capabilities_program):
        self.selected_backend=self.executable('dolgorae', 'import sys,os,time,json\nif sys.argv[1:]==["version","--json"]:\n '+version_program.replace('\n','\n ')+'\nelif sys.argv[1:]==["runtime","capabilities"]:\n '+capabilities_program.replace('\n','\n ')+'\nelse: raise SystemExit(91)')

    def print_version(self): return 'print('+repr(json.dumps(self.version))+')'
    def print_caps(self): return 'print('+repr(json.dumps(self.machine))+')'

    def test_exact_prefix_suffix_and_case(self):
        for version in ['0.1.4-RC.1+Build.007', '0.1.4']:
            with self.subTest(version=version):
                self.caps['dolgorae_version']=version
                self.version['version']='v'+version
                self.fixtures();self.invoke()
        for actual in ['0.1.5-RC.1+Build.007', '0.1.4-rc.1+Build.007', '0.1.4-RC.1+build.007']:
            with self.subTest(actual=actual):
                self.caps['dolgorae_version']='0.1.4-RC.1+Build.007'
                self.version['version']='v'+self.caps['dolgorae_version']
                self.fixtures();self.assertEqual(self.invoke()['checks'][0]['outcome'],'pass')
                self.version['version']='v'+actual
                self.fixtures();self.assertEqual(self.invoke(1)['checks'][0]['outcome'],'fail')

    def test_complete_schema_validation_rejects_nested_missing_and_wrong_command(self):
        for mutate in [lambda d:d['data'].pop('profile'),lambda d:d['data'].update(extra=True),
                       lambda d:d.update(command='run.status'),lambda d:d.update(schema_version=True),
                       lambda d:d.update(invocation_id='00000000-0000-4000-8000-000000000001')]:
            value=copy.deepcopy(self.machine);mutate(value);self.fixtures(capabilities=value)
            self.assertEqual(self.invoke(1)['probes'][2]['outcome'],'fail')

    def test_identity_envelope_and_stateless_version_are_closed(self):
        for value in [{**self.podway,'command':'help'}, {**self.podway,'workspace':{}},
                      {**self.podway,'generated_at':'not-a-timestamp'}]:
            self.fixtures(podway=value);self.invoke(1)
        self.fixtures(version={**self.version,'extra':'not allowed'});self.invoke(1)

    def test_identifiable_unsupported_families(self):
        self.fixtures(capabilities={**self.machine,'schema_version':5});self.invoke(3)
        self.fixtures(podway={**self.podway,'schema':'podway.output/v5'});self.invoke(3)
        self.fixtures(version={**self.machine,'schema_version':1,'command':'version','data':self.version});self.invoke(3)

    def test_supported_malformed_payload_is_not_an_unknown_family(self):
        self.fixtures(version={**self.version,'schema_version':1,'ok':True});self.invoke(1)
        self.fixtures(version={'schema_version':1,'ok':True});self.invoke(1)
        self.fixtures(capabilities={'schema_version':5,'ok':True});self.invoke(1)

    def test_recognized_machine_errors_on_version_do_not_bypass_validation(self):
        for code,details in [('INTERNAL_ERROR',{'invariant':'controlled failure'}),
                             ('PROTOCOL_VERSION_UNSUPPORTED',{'protocol':'machine','requested':99,'supported':[4]})]:
            frame={'schema_version':4,'ok':False,'command':'capabilities','invocation_id':self.machine['invocation_id'],
                   'error':{'code':code,'message':'controlled error','retryable':False,'details':details}}
            self.fixtures(version=frame);self.assertEqual(self.invoke(1)['probes'][1]['outcome'],'fail')
        frame=copy.deepcopy(self.machine);frame['data']={}
        self.fixtures(version=frame);self.invoke(1)

    def test_early_classification_retains_process_observations(self):
        self.fixtures()
        frame={**self.machine,'schema_version':5}
        program='pid=os.fork()\nif not pid:\n os.close(1);time.sleep(10);os._exit(0)\nprint('+repr(json.dumps(frame))+',flush=True)\nos._exit(7)'
        self.backend_program(self.print_version(),program)
        r=self.invoke(3,('--timeout-seconds','3'));p=r['probes'][2]
        self.assertEqual(p['reason'],'deadline');self.assertEqual(p['exit_code'],7)
        self.assertTrue(p['cleanup_complete']);self.assertEqual(p['validation'],'unsupported_machine_family')

    def test_frozen_error_mapping_and_nonmapped_errors(self):
        cases=[('PROTOCOL_VERSION_UNSUPPORTED',{'protocol':'machine','requested':99,'supported':[4]},3),
               ('UNSUPPORTED_SCHEMA_VERSION',{'schema':'machine','requested':99,'supported':[4]},3),
               ('INTERNAL_ERROR',{'invariant':'controlled failure'},1)]
        for code,details,exit_code in cases:
            frame={'schema_version':4,'ok':False,'command':'capabilities','invocation_id':self.machine['invocation_id'],
                   'error':{'code':code,'message':'controlled metadata error','retryable':False,'details':details}}
            self.fixtures(capabilities=frame);r=self.invoke(exit_code)
            self.assertTrue(r['probes'][2]['schema_valid']);self.assertEqual(r['probes'][2]['error_code'],code)
        frame['error']['code']='PROTOCOL_VERSION_UNSUPPORTED'
        self.fixtures(capabilities=frame);self.invoke(1)

    def test_complete_escaped_surrogate_is_invalid_not_missing_prerequisite(self):
        frame={'schema_version':4,'ok':False,'command':'capabilities','invocation_id':self.machine['invocation_id'],
               'error':{'code':'WORKSPACE_STARTUP_UNSUPPORTED','message':'\ud800','retryable':False,
                        'details':{'schema':'dolgorae-workspace-error-details/v1','required_action':'select_supported_runtime',
                                   'phase':'admission','outcome':'not_dispatched','review_id':None,'run_id':None}}}
        valid=copy.deepcopy(frame);valid['error']['message']='controlled valid message'
        self.fixtures(capabilities=valid);self.assertTrue(self.invoke(1)['probes'][2]['schema_valid'])
        self.fixtures(capabilities=frame);r=self.invoke(1)
        self.assertFalse(r['probes'][2]['schema_valid'])

    def test_complete_malformed_and_duplicate_json_fail(self):
        self.fixtures()
        for text in ['{', '{"name":"dolgorae","name":"other","version":"v1.0.0"}', 'NaN', '[]']:
            self.backend_program('print('+repr(text)+')',self.print_caps());self.invoke(1)

    def test_absent_human_error_is_not_unsupported(self):
        self.fixtures();self.backend_program('print("unsupported prose",file=sys.stderr);raise SystemExit(2)', self.print_caps())
        r=self.invoke(4);self.assertEqual(r['probes'][1]['outcome'],'incomplete')
        self.assertNotIn('unsupported prose',json.dumps(r))

    def test_partial_timeout_is_incomplete_and_preserves_prior_checks(self):
        self.fixtures();self.backend_program(self.print_version(),'os.write(1,b"{");time.sleep(10)')
        r=self.invoke(4,('--timeout-seconds','3'));self.assertEqual(r['probes'][0]['outcome'],'pass')
        self.assertEqual(r['probes'][2]['reason'],'deadline');self.assertTrue(r['probes'][2]['cleanup_complete'])

    def test_complete_invalid_output_survives_retained_stderr_timeout(self):
        self.fixtures();self.backend_program(self.print_version(), 'pid=os.fork()\nif not pid:\n os.close(1);time.sleep(10);os._exit(0)\nprint("{",flush=True)\nos._exit(0)')
        self.invoke(1,('--timeout-seconds','3'))

    def test_retained_pipes_have_bounded_owned_group_cleanup(self):
        pidfile=self.root/'child.pid';self.fixtures()
        program='pid=os.fork()\nif not pid: time.sleep(10);os._exit(0)\nopen('+repr(str(pidfile))+',"w").write(str(os.getpgrp()))\n'+self.print_caps()+'\nos._exit(0)'
        self.backend_program(self.print_version(),program);began=time.monotonic();r=self.invoke(4,('--timeout-seconds','3'))
        self.assertLess(time.monotonic()-began,5.5);self.assertTrue(r['probes'][2]['cleanup_complete'])
        import verify_dolgorae_connection as verifier
        self.assertTrue(verifier.owned_group_gone(int(pidfile.read_text())))

    def test_unknown_owned_group_state_is_not_successful_cleanup(self):
        import verify_dolgorae_connection as verifier
        with mock.patch.object(verifier,'owned_group_gone',return_value=None):
            observation, raw=verifier.capture([sys.executable,'-c','print("{}")'],time.monotonic()+2)
        self.assertFalse(observation['cleanup_complete']);self.assertEqual(observation['exit_code'],0)
        self.assertTrue(observation['stdout_complete']);self.assertEqual(raw.strip(),b'{}')

    def test_shared_deadline_is_not_renewed_for_each_child(self):
        self.fixtures()
        self.selected_podway=self.executable('podway', 'import time\ntime.sleep(1.5)\nprint('+repr(json.dumps(self.podway))+')')
        self.backend_program('time.sleep(1.5);'+self.print_version(),self.print_caps())
        began=time.monotonic();self.invoke(4,('--timeout-seconds','3'));self.assertLess(time.monotonic()-began,4.5)

    def test_output_boundaries(self):
        self.fixtures()
        payload=json.dumps(self.machine).encode()
        self.backend_program(self.print_version(),'os.write(1,'+repr(payload)+');os.write(1,b" "*'+str(1048576-len(payload))+');os.write(2,b"x"*65536)')
        r=self.invoke();self.assertEqual(r['probes'][2]['stdout_bytes'],1048576);self.assertEqual(r['probes'][2]['stderr_bytes'],65536)
        for program in ['os.write(1,b"x"*1048577)','os.write(2,b"x"*65537)']:
            self.backend_program(self.print_version(),program);r=self.invoke(1)
            self.assertIn(r['probes'][2]['reason'],['stdout_ceiling','stderr_ceiling'])
            name=r['probes'][2]['reason'].removesuffix('_ceiling');limit={'stdout':1048576,'stderr':65536}[name]
            self.assertEqual(r['probes'][2][name+'_bytes'],limit)
            self.assertEqual(r['probes'][2][name+'_observed_bytes'],limit+1)

    def test_nonzero_complete_success_is_failure(self):
        self.fixtures();self.backend_program(self.print_version(),self.print_caps()+';raise SystemExit(7)')
        r=self.invoke(1);self.assertTrue(r['probes'][2]['schema_valid']);self.assertEqual(r['probes'][2]['exit_code'],7)

    def test_failure_priority_survives_timeout_and_unsupported(self):
        self.fixtures(podway={})
        self.backend_program(self.print_version(),'time.sleep(10)');self.invoke(1,('--timeout-seconds','3'))
        self.fixtures(podway={},capabilities={**self.machine,'schema_version':5});self.invoke(1)
        self.fixtures(capabilities={**self.machine,'schema_version':5});self.selected_podway=self.root/'missing';self.invoke(3)

    def test_missing_and_launch_failure_are_incomplete(self):
        self.fixtures();self.selected_podway=self.root/'missing';self.invoke(4)
        self.selected_podway=self.executable('not-a-program','invalid executable');self.selected_podway.write_text('no executable format')
        self.invoke(4)

    def test_initial_native_executable_changes_fail_and_dominate_unsupported(self):
        import verify_dolgorae_connection as verifier
        for change in ['selected_binding','path_witness','descriptor_witness']:
            for unsupported in [False,True]:
                with self.subTest(change=change,unsupported=unsupported):
                    self.fixtures(capabilities={**self.machine,'schema_version':5} if unsupported else self.machine)
                    selected=self.selected_podway
                    original=selected.stat()
                    replacement=self.root/'initial-replacement'
                    replacement.write_bytes(selected.read_bytes());replacement.chmod(0o700)
                    if change=='selected_binding':
                        selected=self.root/'initial-selected'
                        if selected.is_symlink():selected.unlink()
                        selected.symlink_to(self.selected_podway)
                    actual_read=os.read;changed=[]
                    def replace_during_initial_read(descriptor,size):
                        data=actual_read(descriptor,size)
                        if data and not changed and os.fstat(descriptor).st_ino==original.st_ino:
                            if change=='selected_binding':
                                selected.unlink();selected.symlink_to(replacement)
                            elif change=='path_witness':replacement.replace(selected)
                            else:
                                with selected.open('ab') as stream:stream.write(b'\n# controlled native change\n')
                            changed.append(True)
                        return data
                    output=self.root/('initial-'+change+'-'+str(unsupported)+'.json')
                    with mock.patch.object(verifier.os,'read',side_effect=replace_during_initial_read):
                        status=verifier.main(['--podway',str(selected),'--dolgorae',str(self.selected_backend),'--output',str(output)])
                    self.assertTrue(changed)
                    self.assertEqual(status,1)
                    report=json.loads(output.read_text())
                    self.assertEqual(report['checks'][0]['outcome'],'fail')
                    self.assertEqual(report['probe_outcome'],'fail')
                    self.assertNotIn('podway',report['executables'])
                    if unsupported:self.assertEqual(report['probes'][2]['outcome'],'unsupported')

    def test_initial_unavailable_executable_observations_remain_incomplete(self):
        import verify_dolgorae_connection as verifier
        self.fixtures();self.selected_podway.chmod(0)
        report=self.invoke(4)
        self.assertEqual(report['checks'][0]['outcome'],'incomplete')
        self.selected_podway.chmod(0o700)
        report=verifier.verify(str(self.selected_podway),str(self.selected_backend),0,schemas=verifier.Schemas())
        self.assertEqual(report['probe_outcome'],'incomplete')
        self.assertEqual(report['checks'][0]['outcome'],'incomplete')
        self.assertFalse(report['executables'])

    def test_changed_executable_fails_without_launching_replacement(self):
        self.fixtures();marker=self.root/'replacement-ran'
        self.backend_program(self.print_version()+'\nfrom pathlib import Path\np=Path(__file__);q=p.with_suffix(".new");q.write_text("#!'+sys.executable+'\\nfrom pathlib import Path\\nPath('+repr(str(marker))+').touch()\\n");q.chmod(0o700);q.replace(p)',self.print_caps())
        r=self.invoke(1);self.assertFalse(marker.exists());self.assertEqual(r['checks'][-1]['outcome'],'fail')

    def test_reports_do_not_capture_raw_private_output(self):
        secret='CONTROL_PRIVATE_BODY_892374';self.fixtures()
        self.backend_program(self.print_version(),'print('+repr(secret)+');print('+repr(secret)+',file=sys.stderr)')
        self.assertNotIn(secret,json.dumps(self.invoke(1)))

    def test_exclusive_and_unsafe_outputs_precede_probe_execution(self):
        self.fixtures();marker=self.root/'must-not-run'
        self.selected_podway=self.executable('side-effect', 'open('+repr(str(marker))+',"w").write("bad")')
        existing=self.root/'existing';existing.write_bytes(b'unchanged')
        link=self.root/'link';link.symlink_to(existing)
        directory=self.root/'directory';directory.mkdir()
        for output in [existing,link,directory,ROOT,ROOT/'docs/integ004-forbidden-report.json']:
            r=subprocess.run([sys.executable,str(TOOL),'--podway',str(self.selected_podway),'--dolgorae',str(self.selected_backend),'--output',str(output)],capture_output=True,timeout=3)
            self.assertEqual(r.returncode,2);self.assertFalse(marker.exists())
        self.assertEqual(existing.read_bytes(),b'unchanged');self.assertFalse((ROOT/'docs/integ004-forbidden-report.json').exists())

    def test_argument_timeout_bounds(self):
        self.fixtures()
        for value in ['0','61','1.5','bad']:
            output=self.root/('bad-'+value)
            r=subprocess.run([sys.executable,str(TOOL),'--podway',str(self.selected_podway),'--dolgorae',str(self.selected_backend),'--output',str(output),'--timeout-seconds',value],capture_output=True,timeout=3)
            self.assertEqual(r.returncode,2);self.assertFalse(output.exists())

    def test_offline_prerequisites_missing_corrupt_and_embedded_ids(self):
        import verify_dolgorae_connection as verifier
        schemas=verifier.Schemas()
        self.assertTrue(schemas.valid('capabilities',self.caps))
        lock=json.loads((BUNDLE/'lock.json').read_text())
        self.assertEqual(len(lock['resources']),48)
        embedded=next(e for e in lock['resources'] if e['id'].endswith('specialist-policy-input/v3'))
        self.assertTrue(embedded['pointer']);self.assertEqual(embedded['file'],'dolgorae-workspace-execution-v1.schema.json')
        for mode in ['missing','corrupt','ambiguous']:
            destination=self.root/mode;shutil.copytree(BUNDLE,destination)
            if mode=='missing':(destination/'dolgorae-error-contract-v1.json').unlink()
            elif mode=='corrupt':(destination/'dolgorae-version-v1.schema.json').write_bytes(b'{}')
            else:
                modified=copy.deepcopy(lock);modified['resources'].append(modified['resources'][0]);(destination/'lock.json').write_text(json.dumps(modified))
            with self.assertRaises(verifier.PrerequisiteError):verifier.Schemas(destination)
        with mock.patch.object(verifier,'Schemas',side_effect=verifier.PrerequisiteError('missing library')):
            report=verifier.verify('no-call','no-call',1)
            self.assertEqual(report['probe_outcome'],'incomplete');self.assertFalse(report['probes'])

    def test_unregistered_reference_never_uses_network(self):
        import verify_dolgorae_connection as verifier
        import hashlib
        destination=self.root/'unregistered';shutil.copytree(BUNDLE,destination)
        lock=json.loads((destination/'lock.json').read_text());entry=next(e for e in lock['files'] if e['file']=='dolgorae-capabilities-v1.schema.json')
        doc=json.loads((destination/entry['file']).read_text());doc['properties']['new_ref']={'$ref':'https://invalid.example/must-not-fetch'}
        data=json.dumps(doc).encode();(destination/entry['file']).write_bytes(data);entry['sha256']=hashlib.sha256(data).hexdigest();(destination/'lock.json').write_text(json.dumps(lock))
        with mock.patch('urllib.request.urlopen',side_effect=AssertionError('network forbidden')):
            with self.assertRaises(verifier.PrerequisiteError):verifier.Schemas(destination)

if __name__=='__main__': unittest.main()
