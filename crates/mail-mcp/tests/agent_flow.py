#!/usr/bin/env python3
"""Act as a local MCP agent against a disposable Email store, for UI verification.

Usage: agent_flow.py MAIL_MCP DB STEP [ACCOUNT_ID]

Steps (each prints one JSON object):
  probe    list_accounts with a read-only registration; records a blocked attempt
           when agent access is off.
  request  compose a draft, ask to send it, and ask to archive two conversations.
  send     compose a draft and ask to send it.
  read     read the first demo message.
  long     compose a long draft whose end is far below the preview, and ask to send it.
  undo     ask to undo the latest undoable batch.
  attach   compose a draft with a small text attachment and ask to send it.

Never point this at a real mailbox. It uses only local tools; the demo accounts
have no provider configuration, so nothing can be sent.
"""
import json, os, pathlib, selectors, subprocess, sys, time

BINARY, DB, STEP = pathlib.Path(sys.argv[1]).resolve(), pathlib.Path(sys.argv[2]).resolve(), sys.argv[3]
ACCOUNT = sys.argv[4] if len(sys.argv) > 4 else 'demo:personal'


class Client:
    def __init__(self, flags=()):
        self.proc = subprocess.Popen([str(BINARY), '--db', str(DB), *flags], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.seq = 0
        self.buffer = b''
        self.sel = selectors.DefaultSelector()
        self.sel.register(self.proc.stdout, selectors.EVENT_READ)
        self.call('initialize', {'protocolVersion': '2024-11-05', 'capabilities': {}, 'clientInfo': {'name': 'agent-flow-check', 'version': '1'}})
        self.send({'jsonrpc': '2.0', 'method': 'notifications/initialized'})

    def send(self, obj):
        self.proc.stdin.write((json.dumps(obj) + '\n').encode())
        self.proc.stdin.flush()

    def call(self, method, params):
        self.seq += 1
        seq = self.seq
        self.send({'jsonrpc': '2.0', 'id': seq, 'method': method, 'params': params})
        deadline = time.monotonic() + 10
        while time.monotonic() < deadline:
            if b'\n' not in self.buffer:
                if not self.sel.select(max(0, deadline - time.monotonic())):
                    break
                chunk = os.read(self.proc.stdout.fileno(), 65536)
                if not chunk:
                    raise SystemExit('MCP closed: ' + self.proc.stderr.read().decode())
                self.buffer += chunk
            while b'\n' in self.buffer:
                line, self.buffer = self.buffer.split(b'\n', 1)
                if line.strip():
                    value = json.loads(line)
                    if value.get('id') == seq:
                        return value
        raise SystemExit('MCP request timed out: ' + method)

    def tool(self, name, args=None):
        response = self.call('tools/call', {'name': name, 'arguments': args or {}})
        result = response.get('result', {})
        if response.get('error') or result.get('isError'):
            return {'error': json.dumps(response.get('error') or result.get('content'))}
        value = result.get('structuredContent')
        if isinstance(value, dict) and set(value) == {'result'}:
            value = value['result']
        if isinstance(value, dict) and 'content_trust' in value:
            value = value['data']
        return value

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.proc.terminate()


def draft(client, subject):
    made = client.tool('compose_draft', {'account_id': ACCOUNT, 'to': 'maya@example.org', 'subject': subject,
                                         'body_plain': 'Hi Maya,\n\nCould we move Friday\'s design review to 15:00?\n\nThanks,\nAlex'})
    if 'error' in made:
        return made
    return made['id']


out = {}
if STEP == 'probe':
    client = Client()
    out['list_accounts'] = client.tool('list_accounts')
    client.close()
elif STEP in ('request', 'send'):
    client = Client(('--allow-draft', '--allow-send', '--allow-triage'))
    message = draft(client, 'Design review time')
    out['draft'] = message
    out['send'] = client.tool('send_draft', {'message_id': message}) if isinstance(message, str) else message
    if STEP == 'request':
        out['batch'] = client.tool('batch_triage', {'operation_id': f'agent-flow-{time.time_ns()}', 'kind': 'archive', 'thread_ids': ['demo:2', 'demo:3']})
    client.close()
elif STEP == 'long':
    client = Client(('--allow-draft', '--allow-send'))
    body = 'Short opening line.' + '\n' * 40 + 'TAIL-MARKER: this line is far below the preview.'
    made = client.tool('compose_draft', {'account_id': ACCOUNT, 'to': 'rowan@example.net', 'subject': 'Long note', 'body_plain': body})
    out['draft'] = made.get('id') if isinstance(made, dict) else made
    out['send'] = client.tool('send_draft', {'message_id': out['draft']})
    client.close()
elif STEP == 'attach':
    client = Client(('--allow-draft', '--allow-send'))
    message = draft(client, 'Notes attached')
    out['draft'] = message
    out['attachment'] = client.tool('add_draft_attachment', {'message_id': message, 'filename': 'notes.txt', 'mime_type': 'text/plain', 'content_base64': 'aGVsbG8gZnJvbSB0aGUgYWdlbnQK'})
    out['send'] = client.tool('send_draft', {'message_id': message})
    client.close()
elif STEP == 'undo':
    client = Client(('--allow-triage',))
    latest = client.tool('latest_triage_undo')
    out['latest'] = latest
    if isinstance(latest, dict) and latest.get('operation_id'):
        out['undo'] = client.tool('undo_triage', {'operation_id': latest['operation_id']})
    client.close()
elif STEP == 'read':
    client = Client()
    out['get_message'] = client.tool('get_message', {'message_id': 'demo:1'})
    client.close()
else:
    raise SystemExit(f'unknown step {STEP}')
print(json.dumps(out))
