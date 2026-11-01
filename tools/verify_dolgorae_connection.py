#!/usr/bin/env python3
"""Inspect three explicit public metadata exports; never qualify backend activation."""
from __future__ import annotations

import argparse
import ctypes
import errno
import hashlib
import json
import os
from pathlib import Path
import selectors
import signal
import stat
import subprocess
import sys
import time
import uuid
from urllib.parse import urljoin

ROOT = Path(__file__).resolve().parents[1]
BUNDLE = ROOT / 'contracts/dolgorae-metadata-v1'
UPSTREAM = '5fa36ad896a5bb417bd3ef14e1e4e96caa599d80'
LIMITS = {'stdout': 1024 * 1024, 'stderr': 64 * 1024}
PRIORITY = {'pass': 0, 'incomplete': 1, 'unsupported': 2, 'fail': 3}
EXITS = {'pass': 0, 'fail': 1, 'unsupported': 3, 'incomplete': 4}
PRIMARY = {
    'dolgorae-version-v1.schema.json': 'f1f98c3d5bca2f57c159e88f9239aeb9d56dc9f8d94c2c04ea08f1968107b64f',
    'dolgorae-machine-v4.schema.json': 'c88500ee28e68aaeea6c12944f67d604889d53fa3544943c13b992d0a169a665',
    'dolgorae-capabilities-v2.schema.json': 'adcecc3bfb5346abe0c8217f97318bf6d7eccdf4438d0853a36e8a4154cf4828',
    'dolgorae-error-contract-v4.json': '19eb4af3d76567be12c38ecd27e7928c0d1f576a51a5f066c0981ce63c4d02ac',
}
UNSUPPORTED = {'PROTOCOL_VERSION_UNSUPPORTED', 'UNSUPPORTED_SCHEMA_VERSION'}


class PrerequisiteError(Exception):
    """The complete offline validation prerequisites are unavailable."""


class UsageError(Exception):
    """The caller did not select a safe, exclusively new output."""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def strict_json(data):
    def pairs(items):
        result = {}
        for key, value in items:
            if key in result:
                raise ValueError('duplicate JSON field')
            result[key] = value
        return result
    def nonfinite(_value):
        raise ValueError('nonfinite JSON number')
    return json.loads(data.decode('utf-8'), object_pairs_hook=pairs, parse_constant=nonfinite)


def schema_nodes(value, base='', pointer=''):
    if isinstance(value, dict):
        base = urljoin(base, value['$id']) if '$id' in value else base
        yield value, base, pointer
        for key, child in value.items():
            yield from schema_nodes(child, base, pointer + '/' + key.replace('~', '~0').replace('/', '~1'))
    elif isinstance(value, list):
        for index, child in enumerate(value):
            yield from schema_nodes(child, base, pointer + '/' + str(index))


class Schemas:
    def __init__(self, bundle=BUNDLE, podway_schemas=ROOT / 'assets/schemas'):
        try:
            from jsonschema import Draft202012Validator, FormatChecker, validators, ValidationError
            from referencing import Registry, Resource
            def reject(uri):
                raise PrerequisiteError('unregistered offline resource')
            def utf8_bound(validator, bound, instance, schema):
                if isinstance(instance, str):
                    try:
                        size = len(instance.encode('utf-8'))
                    except UnicodeEncodeError:
                        yield ValidationError('string is not valid UTF-8')
                        return
                    if size > bound:
                        yield ValidationError('UTF-8 byte bound exceeded')
            validator_type = validators.extend(Draft202012Validator, {'x-maxUtf8Bytes': utf8_bound})
            lock = strict_json((bundle / 'lock.json').read_bytes())
            if lock['schema'] != 'podway.dolgorae-metadata-lock/v1' or lock['upstream_commit'] != UPSTREAM:
                raise PrerequisiteError('invalid offline lock')
            documents = []
            resource_map = {}
            file_names = set()
            roots = {'version': 'https://dolgorae.local/schema/version-output/v1',
                     'machine': 'https://dolgorae.local/schema/machine-output/v4',
                     'capabilities': 'https://dolgorae.local/schema/capabilities/v2',
                     'error': 'https://dolgorae.local/schema/error-contract/v4'}
            if lock['roots'] != roots:
                raise PrerequisiteError('offline root families changed')
            for entry in lock['files']:
                name = entry['file']
                if name in file_names or Path(name).name != name or entry['upstream_commit'] != UPSTREAM:
                    raise PrerequisiteError('ambiguous offline file')
                file_names.add(name)
                expected_source = 'docs/protocol/' + ('examples/' if entry['kind'] == 'example' else '') + name
                if entry['upstream_path'] != expected_source:
                    raise PrerequisiteError('offline provenance path mismatch')
                path = bundle / name
                if path.is_symlink() or not path.is_file() or path.stat().st_size > 2 * 1024 * 1024:
                    raise PrerequisiteError('unsafe offline file')
                raw = path.read_bytes()
                if digest(raw) != entry['sha256'] or (name in PRIMARY and digest(raw) != PRIMARY[name]):
                    raise PrerequisiteError('altered offline file')
                if entry['kind'] != 'schema':
                    continue
                document = strict_json(raw)
                Draft202012Validator.check_schema(document)
                documents.append(document)
                for node, uri, pointer in schema_nodes(document):
                    if '$id' in node:
                        if uri in resource_map:
                            raise PrerequisiteError('ambiguous offline resource')
                        resource_map[uri] = {'id': uri, 'file': name, 'pointer': pointer}
            if not set(PRIMARY) <= file_names or sorted(resource_map.values(), key=lambda v: v['id']) != lock['resources']:
                raise PrerequisiteError('incomplete offline resource map')
            registry = Registry(retrieve=reject).with_resources(
                (document['$id'], Resource.from_contents(document)) for document in documents
            ).crawl()
            for document in documents:
                for node, uri, _pointer in schema_nodes(document):
                    if '$ref' in node:
                        registry.resolver(uri).lookup(node['$ref'])
            mapping = {entry['code']: entry for entry in lock['unsupported_error_codes']}
            if set(mapping) != UNSUPPORTED:
                raise PrerequisiteError('unsupported-error mapping drift')
            for code, entry in mapping.items():
                node = registry.resolver(entry['source_id']).lookup('#' + entry['pointer']).contents
                if node['properties']['code']['const'] != code:
                    raise PrerequisiteError('unsupported-error source mismatch')
            # Podway's canonical complete public schemas remain the authority for its identity.
            for path in sorted(podway_schemas.glob('*.schema.json')):
                if path.is_symlink():
                    raise PrerequisiteError('unsafe Podway schema')
                document = strict_json(path.read_bytes())
                registry = registry.with_resource(document['$id'], Resource.from_contents(document))
            registry = registry.crawl()
            self.validators = {}
            for name, uri in {**roots, 'podway': 'urn:podway:schema:output:v4',
                              'podway_error': 'urn:podway:schema:error:v2',
                              'podway_identity': 'urn:podway:schema:version-result:v2'}.items():
                document = registry.resolver().lookup(uri).contents
                self.validators[name] = validator_type(document, registry=registry, format_checker=FormatChecker())
            self.lock_sha256 = digest((bundle / 'lock.json').read_bytes())
        except PrerequisiteError:
            raise
        except Exception as error:
            raise PrerequisiteError('offline validator prerequisites unavailable') from error

    def valid(self, name, value):
        try:
            return self.validators[name].is_valid(value)
        except Exception as error:
            raise PrerequisiteError('offline reference resolution unavailable') from error


def witness(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_mode, metadata.st_size,
            metadata.st_mtime_ns, metadata.st_ctime_ns)


class ExecutableChanged(ValueError):
    """A native observation conclusively detected changed executable identity."""


class Executable:
    def __init__(self, selected, deadline):
        self.selected = Path(os.path.abspath(selected))
        self.path = self.selected.resolve(strict=True)
        self.signature, self.sha256 = self.observe(deadline)

    def observe(self, deadline):
        if self.selected.resolve(strict=True) != self.path:
            raise ExecutableChanged('executable replaced')
        descriptor = os.open(self.path, os.O_RDONLY | os.O_NONBLOCK | os.O_NOFOLLOW)
        try:
            before = os.fstat(descriptor)
            if not stat.S_ISREG(before.st_mode) or before.st_size > 128 * 1024 * 1024 or not os.access(self.path, os.X_OK):
                raise OSError('selected executable is not admissible')
            value = hashlib.sha256()
            while True:
                if time.monotonic() >= deadline:
                    raise TimeoutError('executable observation deadline')
                block = os.read(descriptor, 1024 * 1024)
                if not block:
                    break
                value.update(block)
            if witness(before) != witness(os.fstat(descriptor)) or witness(before) != witness(self.path.stat()):
                raise ExecutableChanged('executable changed')
            if self.selected.resolve(strict=True) != self.path:
                raise ExecutableChanged('executable replaced')
            return witness(before), value.hexdigest()
        finally:
            os.close(descriptor)

    def check(self, deadline):
        try:
            # Metadata differences remain conclusive even if the observation budget expired.
            if self.selected.resolve(strict=True) != self.path or witness(self.path.stat()) != self.signature:
                return 'fail'
            signature, value = self.observe(deadline)
            return 'pass' if (signature, value) == (self.signature, self.sha256) else 'fail'
        except TimeoutError:
            return 'incomplete'
        except (OSError, ValueError, RuntimeError):
            return 'fail'

    def public(self):
        return {'selected_path': str(self.selected), 'resolved_path': str(self.path), 'sha256': self.sha256,
                'native_witness': list(self.signature)}


def owned_group_gone(group):
    """Observe only the group created by this verifier; zombies hold no pipes."""
    if sys.platform == 'darwin':
        try:
            library = ctypes.CDLL('/usr/lib/libproc.dylib', use_errno=True)
        except OSError:
            return None
        library.proc_listpids.argtypes = [ctypes.c_uint32, ctypes.c_uint32, ctypes.c_void_p, ctypes.c_int]
        library.proc_listpids.restype = ctypes.c_int
        library.proc_pidinfo.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_uint64, ctypes.c_void_p, ctypes.c_int]
        library.proc_pidinfo.restype = ctypes.c_int
        members = (ctypes.c_int * 65536)()
        ctypes.set_errno(0)
        size = library.proc_listpids(2, group, members, ctypes.sizeof(members))
        if size < 0 or size >= ctypes.sizeof(members) or (size == 0 and ctypes.get_errno()):
            return None
        for pid in members[:size // ctypes.sizeof(ctypes.c_int)]:
            if not pid:
                continue
            info = ctypes.create_string_buffer(136)
            ctypes.set_errno(0)
            received = library.proc_pidinfo(pid, 3, ctypes.c_uint64(0), info, len(info))
            if received == 0 and ctypes.get_errno() == errno.ESRCH:
                continue
            if received != len(info):
                return None
            fields = (ctypes.c_uint32 * 4).from_buffer_copy(info.raw[:16])
            if fields[3] != pid:
                return None
            if fields[1] != 5:  # Darwin SZOMB: no live process or retained descriptors.
                return False
        return True
    try:
        os.killpg(group, 0)
    except ProcessLookupError:
        return True
    except OSError:
        return None
    return False


def identifiable_machine(value):
    if type(value.get('schema_version')) is not int or value['schema_version'] < 1:
        return False
    if type(value.get('ok')) is not bool or not isinstance(value.get('command'), str) or not value['command']:
        return False
    try:
        identity = value['invocation_id']
        if not isinstance(identity, str) or str(uuid.UUID(identity)) != identity:
            return False
    except (KeyError, ValueError):
        return False
    return ('data' if value['ok'] else 'error') in value


def capture(argv, deadline):
    result = {'argv': argv, 'exit_code': None, 'reason': None, 'stdout_complete': False,
              'cleanup_complete': True, 'termination_requested': False,
              'exit_code_before_cleanup': None, 'stdout_bytes': 0, 'stderr_bytes': 0}
    if time.monotonic() >= deadline:
        result['reason'] = 'deadline'
        return result, b''
    try:
        process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.PIPE, start_new_session=True)
    except OSError:
        result['reason'] = 'launch_failure'
        return result, b''
    selector = selectors.DefaultSelector()
    buffers = {name: bytearray() for name in LIMITS}
    observed = {name: 0 for name in LIMITS}
    try:
        for name, stream in [('stdout', process.stdout), ('stderr', process.stderr)]:
            os.set_blocking(stream.fileno(), False)
            selector.register(stream, selectors.EVENT_READ, name)
        while selector.get_map() and result['reason'] is None:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                result['reason'] = 'deadline'
                break
            for key, _events in selector.select(min(remaining, 0.1)):
                name = key.data
                data = os.read(key.fileobj.fileno(), min(65536, LIMITS[name] + 1 - len(buffers[name])))
                if not data:
                    selector.unregister(key.fileobj)
                    if name == 'stdout':
                        result['stdout_complete'] = True
                else:
                    observed[name] += len(data)
                    available = LIMITS[name] - len(buffers[name])
                    buffers[name].extend(data[:available])
                    if len(data) > available:
                        result['reason'] = name + '_ceiling'
                        break
        if result['reason'] is None:
            try:
                result['exit_code'] = process.wait(timeout=max(0.001, deadline - time.monotonic()))
            except subprocess.TimeoutExpired:
                result['reason'] = 'deadline'
        else:
            result['exit_code'] = process.poll()
    finally:
        result['exit_code_before_cleanup'] = result['exit_code']
        selector.close()
        try:
            os.killpg(process.pid, signal.SIGKILL)
            result['termination_requested'] = True
        except ProcessLookupError:
            pass
        except OSError:
            result['cleanup_complete'] = False
        try:
            result['exit_code'] = process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            result['cleanup_complete'] = False
        for stream in (process.stdout, process.stderr):
            stream.close()
        cleanup_deadline = time.monotonic() + 1
        while True:
            gone = owned_group_gone(process.pid)
            if gone is True:
                break
            if gone is None or time.monotonic() >= cleanup_deadline:
                result['cleanup_complete'] = False
                break
            time.sleep(0.01)
    for name, value in buffers.items():
        result[name + '_bytes'] = len(value)
        result[name + '_observed_bytes'] = observed[name]
        result[name + '_sha256'] = digest(value)
    return result, bytes(buffers['stdout'])


def classify(kind, observation, raw, schemas):
    result = {'name': kind, **observation, 'schema_valid': False, 'outcome': 'incomplete'}
    if observation['reason'] in ('stdout_ceiling', 'stderr_ceiling'):
        result['outcome'] = 'fail'
        return result, None
    if not raw or not observation['stdout_complete']:
        result['validation'] = 'absent_or_incomplete_output'
        return result, None
    try:
        value = strict_json(raw)
    except (ValueError, UnicodeError, RecursionError):
        result.update(outcome='fail', validation='malformed_complete_json')
        return result, None
    if not isinstance(value, dict):
        result.update(outcome='fail', validation='invalid_supported_instance')
        return result, None
    if kind != 'podway' and value.get('schema_version') == 4 and value.get('ok') is False:
        expected_command = 'capabilities' if kind == 'capabilities' else 'version'
        valid = (schemas.valid('machine', value) and schemas.valid('error', value.get('error'))
                 and value.get('command') == expected_command)
        result.update(schema_valid=valid, validation='recognized_error' if valid else 'invalid_supported_error',
                      outcome='unsupported' if valid and value['error']['code'] in UNSUPPORTED else 'fail')
        if valid:
            result['error_code'] = value['error']['code']
        return result, None
    if kind == 'podway':
        schema = value.get('schema')
        if isinstance(schema, str) and schema != 'podway.output/v4' and schema != 'podway.error/v2':
            result.update(outcome='unsupported', validation='unsupported_family')
            return result, None
        if schema == 'podway.error/v2':
            result.update(outcome='fail', schema_valid=schemas.valid('podway_error', value), validation='podway_error')
            return result, None
        family = value.get('result', {}).get('schema') if isinstance(value.get('result'), dict) else None
        if isinstance(family, str) and family.startswith('podway.version-result/') and family != 'podway.version-result/v2':
            result.update(outcome='unsupported', validation='unsupported_identity_family')
            return result, None
        valid = schemas.valid('podway', value) and value.get('command') == 'version' and schemas.valid('podway_identity', value.get('result'))
    elif kind == 'version':
        if 'name' not in value and 'version' not in value and identifiable_machine(value):
            valid = value['schema_version'] != 4 or schemas.valid('machine', value)
            result.update(outcome='unsupported' if valid else 'fail',
                          validation='version_requires_stateless_family' if valid else 'invalid_supported_instance')
            return result, None
        valid = schemas.valid('version', value)
    else:
        if identifiable_machine(value) and value['schema_version'] != 4:
            result.update(outcome='unsupported', validation='unsupported_machine_family')
            return result, None
        valid = schemas.valid('machine', value) and value.get('command') == 'capabilities'
        if value.get('ok') is False:
            valid = valid and schemas.valid('error', value.get('error'))
            result.update(schema_valid=valid, validation='recognized_error' if valid else 'invalid_supported_error',
                          outcome='unsupported' if valid and value['error']['code'] in UNSUPPORTED else 'fail')
            if valid:
                result['error_code'] = value['error']['code']
            return result, None
        valid = valid and schemas.valid('capabilities', value.get('data'))
    result['schema_valid'] = valid
    if not valid:
        result.update(outcome='fail', validation='invalid_supported_instance')
        return result, None
    result['validation'] = 'validated_complete_success'
    natural_exit = observation['exit_code_before_cleanup']
    if natural_exit is None and not observation['termination_requested']:
        natural_exit = observation['exit_code']
    result['outcome'] = 'fail' if natural_exit not in (0, None) else 'pass'
    if result['outcome'] == 'pass' and (observation['reason'] is not None or not observation['cleanup_complete']):
        result['outcome'] = 'incomplete'
    return result, value


class Output:
    def __init__(self, selected):
        self.path = Path(os.path.abspath(selected))
        parent = self.path.parent
        if parent.resolve(strict=True) != parent or not parent.is_dir():
            raise UsageError('output requires an existing canonical directory')
        if self.path.is_relative_to(ROOT) and (self.path == ROOT or self.path.relative_to(ROOT).parts[0] not in ('artifacts', 'dist', 'target')):
            raise UsageError('output must be outside canonical repository trees')
        self.parent = os.open(parent, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
        try:
            self.parent_witness = witness(os.fstat(self.parent))[:3]
            if os.fstat(self.parent).st_uid != os.geteuid():
                raise UsageError('output directory must be caller-owned')
            self.descriptor = os.open(self.path.name, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600, dir_fd=self.parent)
        except BaseException:
            os.close(self.parent)
            raise

    def write(self, report):
        try:
            if witness(self.path.parent.stat())[:3] != self.parent_witness or witness(os.stat(self.path.name, dir_fd=self.parent, follow_symlinks=False))[:2] != witness(os.fstat(self.descriptor))[:2]:
                raise UsageError('output binding changed')
            with os.fdopen(self.descriptor, 'w', encoding='utf-8') as stream:
                self.descriptor = None
                json.dump(report, stream, indent=2)
                stream.write('\n')
        finally:
            if self.descriptor is not None:
                os.close(self.descriptor)
            os.close(self.parent)


def verify(podway, backend, timeout, schemas=None):
    deadline = time.monotonic() + timeout
    report = {'schema': 'podway.dolgorae-connection-report/v1', 'probe_outcome': 'incomplete',
              'activation_qualified': False, 'executables': {}, 'probes': [], 'checks': [],
              'bounds': {'timeout_seconds': timeout, 'stdout_bytes': LIMITS['stdout'],
                         'stderr_bytes': LIMITS['stderr'], 'executable_bytes': 128 * 1024 * 1024,
                         'leader_reap_seconds': 1, 'group_observation_seconds': 1},
              'limits': ['Public metadata only; no task/result/receipt, worker disposition or activation qualification.',
                         'Only verifier-owned process groups are terminated; cleanup has a separate bounded reaping allowance.',
                         'Executable witnesses are observations, not a security boundary against same-user replacement.',
                         'Darwin cleanup observes only the owned group via libproc; zombie-only groups are non-live. Unknown group state is incomplete.']}
    try:
        schemas = schemas or Schemas()
        report['validation_lock_sha256'] = schemas.lock_sha256
    except PrerequisiteError:
        report['checks'].append({'name': 'offline_prerequisites', 'outcome': 'incomplete'})
        return report
    pins = {}
    for name, path in [('podway', podway), ('dolgorae', backend)]:
        try:
            pins[name] = Executable(path, deadline)
            report['executables'][name] = pins[name].public()
        except ExecutableChanged:
            report['checks'].append({'name': name + '_executable', 'outcome': 'fail', 'reason': 'executable_changed'})
        except (OSError, ValueError, RuntimeError, TimeoutError):
            report['checks'].append({'name': name + '_executable', 'outcome': 'incomplete'})
    values = {}
    for kind, selected, arguments in [('podway', 'podway', ['--json', 'version', '--identity']),
                                      ('version', 'dolgorae', ['version', '--json']),
                                      ('capabilities', 'dolgorae', ['runtime', 'capabilities'])]:
        pin = pins.get(selected)
        if pin is None:
            report['probes'].append({'name': kind, 'outcome': 'incomplete', 'reason': 'missing_executable'})
            continue
        checked = pin.check(deadline)
        if checked != 'pass':
            report['probes'].append({'name': kind, 'outcome': checked, 'reason': 'executable_witness'})
            continue
        observation, raw = capture([str(pin.path), *arguments], deadline)
        try:
            result, value = classify(kind, observation, raw, schemas)
        except PrerequisiteError:
            result, value = {'name': kind, **observation, 'outcome': 'incomplete', 'validation': 'offline_reference_unavailable'}, None
        report['probes'].append(result)
        if value is not None:
            values[kind] = value
    if 'version' in values and 'capabilities' in values:
        equal = values['version']['version'] == 'v' + values['capabilities']['data']['dolgorae_version']
        report['checks'].append({'name': 'exact_version_comparison', 'outcome': 'pass' if equal else 'fail',
                                 'version_output_sha256': digest(values['version']['version'].encode()),
                                 'capabilities_version_sha256': digest(values['capabilities']['data']['dolgorae_version'].encode())})
        if max(len(values['version']['version']), len(values['capabilities']['data']['dolgorae_version'])) <= 256:
            report['versions'] = {'version_output': values['version']['version'], 'capabilities': values['capabilities']['data']['dolgorae_version']}
    else:
        report['checks'].append({'name': 'exact_version_comparison', 'outcome': 'incomplete', 'reason': 'both_contracts_required'})
    if 'podway' in values:
        report['podway_identity'] = values['podway']['result']
    for name, pin in pins.items():
        report['checks'].append({'name': name + '_final_executable_witness', 'outcome': pin.check(deadline)})
    report['probe_outcome'] = max((entry['outcome'] for entry in report['checks'] + report['probes']), key=PRIORITY.get)
    return report


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--podway', required=True)
    parser.add_argument('--dolgorae', required=True)
    parser.add_argument('--output', required=True)
    parser.add_argument('--timeout-seconds', type=int, choices=range(1, 61), default=15)
    args = parser.parse_args(argv)
    try:
        output = Output(args.output)
    except (OSError, RuntimeError, UsageError):
        parser.error('output must be a new file in a caller-owned canonical evidence directory')
    report = verify(args.podway, args.dolgorae, args.timeout_seconds)
    try:
        output.write(report)
    except (OSError, UsageError):
        print('output binding changed or report could not be written', file=sys.stderr)
        return 2
    print(json.dumps({'probe_outcome': report['probe_outcome'], 'activation_qualified': False}))
    return EXITS[report['probe_outcome']]


if __name__ == '__main__':
    raise SystemExit(main())
