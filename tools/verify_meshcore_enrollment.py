"""Exercise enrollment against local backbone, host and radio worker processes."""

import argparse
from pathlib import Path
import json
import socket
import struct
import subprocess
import tempfile
import time
import urllib.parse
import urllib.request
from http.cookiejar import CookieJar
from websockets.sync.client import connect

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('host_bin_dir', type=Path, help='Directory with the M?:rse host and server binaries')
parser.add_argument('meshcore_worker', type=Path, help='Path to the MeshCore radio worker binary')
args = parser.parse_args()
BIN = args.host_bin_dir.resolve()
WORKER = args.meshcore_worker.resolve()


def port():
    with socket.socket() as listener:
        listener.bind(('127.0.0.1', 0))
        return listener.getsockname()[1]


def local(address):
    return f'127.0.0.1:{address}'


def start_ready(address):
    for _ in range(120):
        try:
            with socket.create_connection(('127.0.0.1', address), timeout=1):
                return
        except ConnectionRefusedError:
            time.sleep(.1)
    raise RuntimeError('Listener not ready')


def command(ws, operation, request, data=b''):
    frame = struct.pack('<BHB', 0x37, request, operation) + data
    ws.send(struct.pack('<H', len(frame)) + frame)


def events(ws):
    while True:
        body = ws.recv(timeout=8)
        while body:
            length = int.from_bytes(body[:2], 'little')
            yield body[2:2 + length]
            body = body[2 + length:]


with tempfile.TemporaryDirectory() as folder, socket.socket() as radio:
    data = Path(folder)
    host_port, server_port = port(), port()
    server_data, host_data = data/'server', data/'host'
    sources = []
    def spawn(args, **options):
        child = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **options)
        sources.append(child)
        return child
    try:
        spawn([BIN/'maarse-server.exe', '--data', server_data, '--http', local(server_port), '--listen', 'off',
               '--public-addr', '127.0.0.1:4242'])
        spawn([BIN/'maarse-host.exe', '--data', host_data, '--http', local(host_port), '--listen', 'off', '--service'])
        start_ready(server_port)
        start_ready(host_port)
        radio.bind(('127.0.0.1', 0))
        radio.listen()
        radio.settimeout(10)
        spawn([WORKER, '--service', '--tcp', local(radio.getsockname()[1]), '--data', server_data/'meshcore-nodes'], stdin=subprocess.PIPE)
        backing, _ = radio.accept()
        with backing, connect(f'ws://{local(host_port)}/ws') as host:
            frames = events(host)
            command(host, 1, 42)
            person = next(event for event in frames if event[0] == 0xb6 and event[1] == 2)[3:19].hex()
            browser = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(CookieJar()))
            admin = (server_data/'admin-password.txt').read_text().splitlines()[0]
            base = f'http://{local(server_port)}'
            browser.open(base+'/login', urllib.parse.urlencode(dict(password=admin)).encode()).close()
            def grant(action):
                browser.open(base+'/meshcore-access', urllib.parse.urlencode(dict(person=person, action=action)).encode()).close()
            def request(operation=0, payload=""):
                with connect(f'ws://{local(server_port)}/meshcore/session') as session:
                    hello = json.loads(session.recv(timeout=10))
                    command(host, 10, 43, bytes([operation]) + bytes.fromhex(hello['challenge']) + payload.encode())
                    proof = bytearray()
                    while True:
                        event = next(frames)
                        if event[0] != 0xbc:
                            continue
                        req, offset, total = struct.unpack('<HHH', event[1:7])
                        assert req == 43 and offset == len(proof)
                        proof.extend(event[7:])
                        if len(proof) == total:
                            break
                    session.send(json.dumps(dict(device=proof[:64].hex(), certificate=proof[64:193].hex(),
                                                 signature=proof[193:257].hex(), list=proof[257:].hex(),
                                                 operation=payload or ['access', 'ensure', 'announce', 'scan', 'scan-results', 'public-history', '', '', 'direct-list', '', '', '', 'channel-list'][operation])))
                    return json.loads(session.recv(timeout=10))
            assert request() == {'allowed': False}
            grant('grant')
            assert request() == {'allowed': True, 'person': person, 'publicKey': None}
            first = request(1)
            assert first['allowed'] and len(first['publicKey']) == 64, first
            assert request() == first and request(1) == first
            assert request(2)['queued'] == 'announce'
            assert request(3)['queued'] == 'scan'
            assert request(4)['scanning'] is True
            for _ in range(15):
                time.sleep(1)
                scan = request(4)
                if 'results' in scan:
                    break
            else:
                raise AssertionError('Radio worker did not publish scan results')
            assert scan['results'] == []
            assert request(6, 'public-send:' + b'hello MeshCore'.hex())['queued'] == 'public-send'
            for _ in range(12):
                history = request(5)
                if history['messages']:
                    break
                time.sleep(.25)
            assert history['messages'][0]['text'] == 'hello MeshCore' and history['messages'][0]['mine']
            node = server_data/'meshcore-nodes'/person
            with (node/'public-history.txt').open('a', encoding='ascii') as history_file:
                history_file.write('in 42 ' + b'test incoming'.hex() + '\n')
            history = request(5)
            assert history['unread'] == 1 and history['messages'][-1]['text'] == 'test incoming'
            assert request(7, f"public-read:{history['lastId']}")['unread'] == 0
            assert request(5)['unread'] == 0
            contact = '12345678' * 8
            assert request(10, f'direct-send:{contact}:' + b'direct test'.hex())['queued'] == 'direct-send'
            for _ in range(12):
                time.sleep(.25)
                outcome = request(9, f'direct-history:{contact}')
                if 'error' in outcome:
                    break
            assert 'Contact is unknown' in outcome['error']
            direct_log = node/f'direct-{contact}-history.txt'
            direct_log.write_text('in 43 ' + b'private incoming'.hex() + '\n', encoding='ascii')
            assert request(8)['contacts'] == [contact]
            private = request(9, f'direct-history:{contact}')
            assert request(5)['totalUnread'] == 1
            assert private['unread'] == 1 and private['messages'][0]['text'] == 'private incoming'
            assert request(11, f"direct-read:{contact}:{private['lastId']}")['unread'] == 0
            assert request(5)['totalUnread'] == 0
            assert request(12)['channels'] == []
            assert request(13, 'channel-join:smoke:')['channels'] == ['smoke']
            assert request(12)['channels'] == ['smoke']
            assert request(16, 'channel-send:smoke:' + b'channel test'.hex())['queued'] == 'channel-send'
            for _ in range(12):
                time.sleep(.25)
                channel = request(15, 'channel-history:smoke')
                if channel['messages']:
                    break
            assert channel['messages'][0]['text'] == 'channel test'
            assert request(14, 'channel-leave:smoke')['channels'] == []
            assert 'Join the channel' in request(16, 'channel-send:smoke:' + b'left'.hex())['error']
            repeater = 'abcdef12' * 8
            assert request(20, f'repeater-status:{repeater}')['repeaterStatus'] == 'Login required.'
            assert request(18, f'repeater-login:{repeater}:' + b'ephemeral-test'.hex())['queued'] == 'repeater-login'
            assert not (node/'repeater-job.txt').exists()
            assert 'Repeater is unknown' in request(20, f'repeater-status:{repeater}')['repeaterStatus']
            grant('revoke')
            assert request(2) == {'allowed': False}
            assert request() == {'allowed': False}
            print('Backbone permission, companion proof, worker enrollment, public key reuse, scan completion, Public and direct history, channel membership, repeater request handling, read states and revocation passed.')
    finally:
        for child in reversed(sources):
            child.terminate()
            child.wait(timeout=10)
