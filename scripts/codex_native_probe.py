#!/usr/bin/env python3
"""Disposable native Codex continuity experiment; synthetic model, no credentials.

Only creates fresh private temporary homes. Never accepts an existing Codex home.
Report contains structural/hash/row-change evidence, never transcript payloads.
"""
import argparse
import datetime
import hashlib
import http.server
import json
import os
from pathlib import Path
import platform
import queue
import shutil
import sqlite3
import subprocess
import tempfile
import threading
import time


def digest(data):
    return hashlib.sha256(data).hexdigest()


def inventory(home):
    result = {}
    for path in sorted(home.rglob('*')):
        if not path.is_file() or path.is_symlink():
            continue
        relative = str(path.relative_to(home))
        if path.name in ('auth.json', 'config.toml'):
            result[relative] = {'excluded': True}
            continue
        data = path.read_bytes()
        item = {'size': len(data), 'sha256': digest(data)}
        if path.suffix == '.sqlite':
            connection = sqlite3.connect(f'file:{path}?mode=ro', uri=True)
            try:
                tables = {}
                for (name,) in connection.execute("SELECT name FROM sqlite_master WHERE type='table'"):
                    quoted = '"' + name.replace('"', '""') + '"'
                    rows = connection.execute(f'SELECT * FROM {quoted}').fetchall()
                    # Hash rows, never report transcript-bearing SQLite values.
                    hashes = sorted(digest(repr(row).encode()) for row in rows)
                    tables[name] = {'count': len(rows), 'row_hashes': hashes}
                item['tables'] = tables
            finally:
                connection.close()
        result[relative] = item
    return result


def difference(before, after):
    return {'created': {k: v for k, v in after.items() if k not in before},
            'modified': {k: {'before': before[k], 'after': v} for k, v in after.items()
                         if k in before and before[k] != v},
            'removed': sorted(set(before) - set(after))}


class SyntheticModel(http.server.BaseHTTPRequestHandler):
    requests = []
    pending_tool = True

    def log_message(self, *_args):
        pass

    def do_POST(self):
        size = int(self.headers.get('Content-Length', '0'))
        raw = self.rfile.read(size)
        body = json.loads(raw)
        encoded = json.dumps(body)
        inputs = body.get('input', [])
        self.requests.append({
            'has_initial_prompt': 'AGENTSYNC_INITIAL_MARKER' in encoded,
            'has_second_prompt': 'AGENTSYNC_SECOND_MARKER' in encoded,
            'has_prior_assistant': 'AGENTSYNC_SYNTHETIC_ACK' in encoded,
            'has_tool_result': any(i.get('type') == 'function_call_output' for i in inputs),
            'input_count': len(inputs),
            'request_sha256': digest(raw),
        })
        number = len(self.requests)
        if type(self).pending_tool:
            type(self).pending_tool = False
            item = {'type': 'function_call', 'id': f'fc_{number}',
                    'call_id': f'call_{number}', 'name': 'exec_command',
                    'arguments': json.dumps({'cmd': 'cat continuity-note.txt', 'login': False}),
                    'status': 'completed'}
        else:
            item = {'id': f'msg_{number}', 'type': 'message', 'role': 'assistant',
                    'status': 'completed', 'phase': 'final_answer',
                    'content': [{'type': 'output_text', 'text': 'AGENTSYNC_SYNTHETIC_ACK',
                                 'annotations': []}]}
        response_id = f'resp_{number}'
        events = [
            {'type': 'response.created', 'response': {'id': response_id, 'status': 'in_progress', 'output': []}},
            {'type': 'response.output_item.added', 'output_index': 0, 'item': item},
            {'type': 'response.output_item.done', 'output_index': 0, 'item': item},
            {'type': 'response.completed', 'response': {'id': response_id, 'status': 'completed',
              'output': [item], 'usage': {'input_tokens': 10, 'output_tokens': 2, 'total_tokens': 12}}},
        ]
        data = ''.join('event: ' + e['type'] + '\ndata: ' + json.dumps(e) + '\n\n' for e in events).encode()
        self.send_response(200)
        self.send_header('Content-Type', 'text/event-stream')
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


class Probe:
    def __init__(self, executable, root, server):
        self.executable, self.root = executable, root
        self.steps = []
        self.base = []
        for setting in [
            'model_provider="agentsync_synthetic"', 'model="mock-model"',
            'model_providers.agentsync_synthetic.name="AgentSync synthetic endpoint"',
            f'model_providers.agentsync_synthetic.base_url="http://127.0.0.1:{server.server_port}/v1"',
            'model_providers.agentsync_synthetic.wire_api="responses"',
            'model_providers.agentsync_synthetic.requires_openai_auth=false',
            'model_providers.agentsync_synthetic.supports_websockets=false',
            'features.shell_snapshot=false', 'analytics.enabled=false',
        ]:
            self.base.extend(['-c', setting])

    def env(self, home):
        home.mkdir(parents=True, exist_ok=True, mode=0o700)
        user = self.root / 'user-home'
        user.mkdir(exist_ok=True, mode=0o700)
        return {'PATH': '/usr/local/bin:/usr/bin:/bin', 'HOME': str(user),
                'CODEX_HOME': str(home), 'SHELL': '/bin/bash', 'LANG': 'C.UTF-8',
                'GIT_CONFIG_NOSYSTEM': '1', 'GIT_CONFIG_GLOBAL': '/dev/null',
                'GIT_OPTIONAL_LOCKS': '0', 'NO_COLOR': '1'}

    def exec(self, name, home, repo, prompt, session=None):
        env = self.env(home)
        before = inventory(home)
        command = [self.executable, *self.base, 'exec', '-C', str(repo)]
        if session:
            command += ['resume', session]
        command += ['--ignore-user-config', '--ignore-rules', '--json', prompt]
        request_start = len(SyntheticModel.requests)
        result = subprocess.run(command, cwd=repo, env=env, stdin=subprocess.DEVNULL,
                                capture_output=True, text=True, timeout=45)
        events = []
        for line in result.stdout.splitlines():
            try:
                events.append(json.loads(line))
            except ValueError:
                pass
        found = next((e['thread_id'] for e in events if e.get('type') == 'thread.started'), None)
        step = {'name': name, 'exit_code': result.returncode, 'session_id': found,
                'event_types': [e.get('type') for e in events],
                'completed': any(e.get('type') == 'turn.completed' for e in events),
                'tool_completed': any(e.get('item', {}).get('type') == 'command_execution'
                                      and e.get('item', {}).get('exit_code') == 0 for e in events),
                'requests': SyntheticModel.requests[request_start:],
                'filesystem': difference(before, inventory(home))}
        self.steps.append(step)
        print(name, 'exit', result.returncode, 'complete', step['completed'], flush=True)
        if result.returncode:
            print('  Native command failed; stderr SHA256:', digest(result.stderr.encode()), flush=True)
        return found, step

    def rpc(self, name, home, repo, session, resume=False):
        env = self.env(home)
        before = inventory(home)
        process = subprocess.Popen([self.executable, *self.base, 'app-server', '--stdio'],
            cwd=repo, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL, text=True, bufsize=1)
        responses = queue.Queue()
        def read():
            for line in process.stdout:
                try:
                    responses.put(json.loads(line))
                except ValueError:
                    pass
        threading.Thread(target=read, daemon=True).start()
        counter = 0
        def request(method, params):
            nonlocal counter
            counter += 1
            process.stdin.write(json.dumps({'id': counter, 'method': method, 'params': params}) + '\n')
            process.stdin.flush()
            deadline = time.monotonic() + 20
            while True:
                message = responses.get(timeout=max(0.01, deadline - time.monotonic()))
                if message.get('id') == counter:
                    return message
        step = {'name': name, 'discovered': False}
        try:
            request('initialize', {'clientInfo': {'name': 'agentsync_probe', 'version': '1'},
                                   'capabilities': {'experimentalApi': True}})
            listed = request('thread/list', {'sourceKinds': ['exec'], 'modelProviders': [],
                                            'useStateDbOnly': False, 'limit': 100})
            threads = listed.get('result', {}).get('data', [])
            matched = [t for t in threads if t.get('id') == session]
            read = request('thread/read', {'threadId': session, 'includeTurns': True})
            thread = read.get('result', {}).get('thread', {})
            step = {'name': name, 'discovered': bool(matched),
                    'discovered_cwd': matched[0].get('cwd') if matched else None,
                    'read_ok': 'result' in read, 'read_error_code': read.get('error', {}).get('code'),
                    'turn_count': len(thread.get('turns', []))}
            if resume:
                resumed = request('thread/resume', {'threadId': session, 'cwd': str(repo),
                    'runtimeWorkspaceRoots': [str(repo)], 'excludeTurns': True})
                result = resumed.get('result', {})
                step['resume_ok'] = 'result' in resumed
                step['resume_cwd'] = result.get('cwd')
                step['resume_thread_cwd'] = result.get('thread', {}).get('cwd')
            return step
        finally:
            process.stdin.close()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                process.terminate()
                process.wait(timeout=5)
            step['filesystem'] = difference(before, inventory(home))
            self.steps.append(step)
            print(name, {k: v for k, v in step.items() if k != 'filesystem'}, flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--codex', default='codex')
    parser.add_argument('--report', type=Path, help='Write full structural evidence JSON here.')
    parser.add_argument('--summary', type=Path, help='Write compact structural evidence JSON here.')
    args = parser.parse_args()
    executable = shutil.which(args.codex)
    if not executable:
        parser.error('Codex executable not found')
    root = Path(tempfile.mkdtemp(prefix='agentsync-codex-native-'))
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), SyntheticModel)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    probe = Probe(executable, root, server)
    source, target = root / 'source-codex', root / 'target-codex'
    source_repo, target_repo = root / 'source-repo', root / 'target-repo'
    source_repo.mkdir()
    (source_repo / 'continuity-note.txt').write_text('Disposable AgentSync continuity experiment.\n')
    env = probe.env(source)
    def git(*cmd, cwd=source_repo):
        return subprocess.check_output(['git', '-c', 'core.hooksPath=/dev/null', *cmd],
                cwd=cwd, env=env, stderr=subprocess.DEVNULL, text=True).strip()
    git('init', '-b', 'continuity-test')
    git('add', 'continuity-note.txt')
    git('-c', 'user.name=AgentSync synthetic probe', '-c', 'user.email=probe@example.invalid',
        'commit', '-m', 'Synthetic continuity fixture')
    git('remote', 'add', 'origin', 'https://example.invalid/agentsync/continuity.git')
    git('clone', '--no-hardlinks', str(source_repo), str(target_repo), cwd=root)
    git('remote', 'set-url', 'origin', 'https://example.invalid/agentsync/continuity.git', cwd=target_repo)
    version = subprocess.check_output([executable, '--version'], env=env, text=True).strip()
    session, first = probe.exec('source-first-turn', source, source_repo, 'AGENTSYNC_INITIAL_MARKER: read continuity-note.txt')
    assert first['completed'] and first['tool_completed'], 'Source tool turn failed'
    _, second = probe.exec('source-second-turn', source, source_repo, 'AGENTSYNC_SECOND_MARKER: acknowledge previous work', session)
    assert second['completed'], 'Source second turn failed'
    rollouts = list((source / 'sessions').rglob('*.jsonl'))
    assert len(rollouts) == 1
    rollout = rollouts[0]
    original_bytes = rollout.read_bytes()
    relative = rollout.relative_to(source)
    installed = target / relative
    installed.parent.mkdir(parents=True, mode=0o700)
    installed.write_bytes(original_bytes)
    transfer_inventory = inventory(target)
    assert list(transfer_inventory) == [str(relative)], 'Target was not rollout-only'
    probe.rpc('rollout-only-discovery', target, target_repo, session)
    # Failure experiment owns this entire fresh home, so removing it restores absence.
    # This is not a claim that deleting a rollout rolls back a shared/live home.
    rollback_home = root / 'rollback-target-codex'
    rollback_rollout = rollback_home / relative
    rollback_rollout.parent.mkdir(parents=True, mode=0o700)
    rollback_rollout.write_bytes(original_bytes)
    rollback_step = probe.rpc('rollback-before-injected-failure', rollback_home, target_repo, session)
    assert rollback_step['discovered'], 'Rollback setup did not reach native discovery'
    state_before_rollback = inventory(rollback_home)
    shutil.rmtree(rollback_home)  # Simulated verification failure: discard owned private home.
    assert not rollback_home.exists(), 'Private-home rollback left state behind'
    probe.steps.append({'name': 'private-home-injected-verification-failure',
        'rollback_strategy': 'discard entire freshly owned disposable target home',
        'rollback_restored_absence': True,
        'filesystem': difference(state_before_rollback, {})})
    source_repo.rename(root / 'source-repo-offline')
    # AgentSync logical objects do not retain original native filenames. Prove
    # a UTC layout reconstructed only from authoritative header fields works.
    header = json.loads(original_bytes.splitlines()[0])['payload']
    stamp = datetime.datetime.fromisoformat(header['timestamp'].replace('Z', '+00:00'))
    stamp = stamp.astimezone(datetime.timezone.utc)
    rebuilt_relative = Path('sessions') / stamp.strftime('%Y/%m/%d') / (
        'rollout-' + stamp.strftime('%Y-%m-%dT%H-%M-%S') + '-' + header['id'] + '.jsonl')
    rebuilt_home = root / 'utc-layout-target-codex'
    rebuilt_rollout = rebuilt_home / rebuilt_relative
    rebuilt_rollout.parent.mkdir(parents=True, mode=0o700)
    rebuilt_rollout.write_bytes(original_bytes)
    rebuilt_discovery = probe.rpc('utc-reconstructed-layout-discovery', rebuilt_home, target_repo, session)
    assert rebuilt_discovery['discovered'], 'Reconstructed UTC layout not discoverable'
    _, rebuilt_turn = probe.exec('utc-reconstructed-layout-resume', rebuilt_home, target_repo,
                                'AGENTSYNC_UTC_LAYOUT_MARKER: continue', session)
    assert rebuilt_turn['completed'] and rebuilt_turn['requests'][-1]['has_second_prompt']
    probe.rpc('utc-reconstructed-layout-restart-discovery', rebuilt_home, target_repo, session)
    _, rebuilt_restart = probe.exec('utc-reconstructed-layout-restart-resume', rebuilt_home, target_repo,
                                   'AGENTSYNC_UTC_RESTART_MARKER: continue again', session)
    assert rebuilt_restart['completed'] and rebuilt_rollout.read_bytes().startswith(original_bytes)
    # Independent target proves exec resume does not require prior app-server repair.
    direct_home = root / 'direct-target-codex'
    direct_rollout = direct_home / relative
    direct_rollout.parent.mkdir(parents=True, mode=0o700)
    direct_rollout.write_bytes(original_bytes)
    _, direct = probe.exec('rollout-only-direct-resume', direct_home, target_repo,
                          'AGENTSYNC_DIRECT_MARKER: continue previous work', session)
    assert direct['completed'], 'Direct rollout-only continuation failed'
    assert direct['requests'][-1]['has_initial_prompt'] and direct['requests'][-1]['has_second_prompt']
    assert direct_rollout.read_bytes().startswith(original_bytes)
    probe.rpc('native-cwd-remap', target, target_repo, session, resume=True)
    _, third = probe.exec('target-continued-turn', target, target_repo, 'AGENTSYNC_TARGET_MARKER: recall previous work', session)
    assert third['completed'], 'Target continuation failed'
    assert third['requests'][-1]['has_initial_prompt'] and third['requests'][-1]['has_second_prompt']
    assert third['requests'][-1]['has_prior_assistant'] and third['requests'][-1]['has_tool_result']
    probe.rpc('target-restart-discovery', target, target_repo, session)
    _, fourth = probe.exec('target-restart-turn', target, target_repo, 'AGENTSYNC_RESTART_MARKER: continue again', session)
    assert fourth['completed'], 'Target restart continuation failed'
    resumed_bytes = installed.read_bytes()
    assert resumed_bytes.startswith(original_bytes), 'Original rollout bytes changed'
    contexts = [e['payload'] for e in map(json.loads, resumed_bytes.splitlines()) if e.get('type') == 'turn_context']
    assert contexts[-1]['cwd'] == str(target_repo), 'Target cwd did not persist'
    report = {'format_version': 1, 'recorded_at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()), 'model_backend': 'deterministic loopback synthetic Responses SSE',
        'certification': 'NOT cross-platform or live-model certification',
        'version': version, 'os': platform.system(), 'architecture': platform.machine(),
        'root': str(root), 'source_home': str(source), 'target_home': str(target),
        'session_id': session, 'source_workspace': str(source_repo), 'target_workspace': str(target_repo),
        'git': {'identity': 'example.invalid/agentsync/continuity', 'branch': 'continuity-test',
                'head': git('rev-parse', 'HEAD', cwd=target_repo)},
        'transferred_files': transfer_inventory,
        'utc_reconstructed_native_path': str(rebuilt_relative),
        'utc_reconstructed_path_differs_from_original': rebuilt_relative != relative, 'rollout_append_preserved_prefix': True,
        'latest_context_cwd': contexts[-1]['cwd'], 'steps': probe.steps}
    report_path = args.report or root / 'report.json'
    report_path.write_text(json.dumps(report, indent=2) + '\n')
    if args.summary:
        compact = json.loads(json.dumps(report))
        for step in compact['steps']:
            changes = step['filesystem']
            tables = {}
            for kind in ('created', 'modified'):
                for name, value in changes[kind].items():
                    after = value if kind == 'created' else value['after']
                    before = {} if kind == 'created' else value['before']
                    if 'tables' in after:
                        tables[name] = {}
                        for table, state in after['tables'].items():
                            previous = before.get('tables', {}).get(table, {})
                            if table.startswith('_') or (state['count'] == 0 and previous.get('count', 0) == 0):
                                continue
                            current_rows = set(state['row_hashes'])
                            previous_rows = set(previous.get('row_hashes', []))
                            tables[name][table] = {'count': state['count'],
                                'added_row_hashes': len(current_rows - previous_rows),
                                'removed_row_hashes': len(previous_rows - current_rows)}
            step['filesystem'] = {
                'created_files': [p for p in changes['created'] if not p.startswith('skills/')],
                'created_bundled_skill_files': sum(p.startswith('skills/') for p in changes['created']),
                'modified_files': list(changes['modified']),
                'removed_files': [p for p in changes['removed'] if not p.startswith('skills/')],
                'removed_bundled_skill_files': sum(p.startswith('skills/') for p in changes['removed']),
                'sqlite_tables': tables,
            }
        args.summary.write_text(json.dumps(compact, indent=2) + '\n')
    print('Evidence:', report_path)
    print('Disposable state:', root)
    server.shutdown()


if __name__ == '__main__':
    main()
