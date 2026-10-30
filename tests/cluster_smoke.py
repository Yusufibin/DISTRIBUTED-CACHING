#!/usr/bin/env python3
"""Exercise real HTTP listeners, routing, replication, snapshots and restarts."""
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

BINARY = Path(os.environ.get('CACHE_NODE_BIN', 'target/debug/cache-node')).resolve()

def free_ports(count):
    sockets = []
    try:
        for _ in range(count):
            sock = socket.socket()
            sock.bind(('127.0.0.1', 0))
            sockets.append(sock)
        return [sock.getsockname()[1] for sock in sockets]
    finally:
        for sock in sockets:
            sock.close()

def request(port, path, method='GET', body=None):
    if isinstance(body, dict):
        body = json.dumps(body).encode()
    req = urllib.request.Request(f'http://127.0.0.1:{port}{path}', data=body, method=method,
                                 headers={'Content-Type': 'application/json'})
    try:
        with urllib.request.urlopen(req, timeout=15) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()

def eventually(check, timeout=10):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if check():
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.05)
    raise AssertionError('condition did not become true')

def main():
    ports = free_ports(6)
    processes, logs = [], []
    with tempfile.TemporaryDirectory(prefix='cache-cluster-') as directory:
        def start(i):
            log = open(Path(directory) / f'node-{i}.log', 'ab')
            logs.append(log)
            proc = subprocess.Popen([str(BINARY), '--node-id', f'custom-{i}',
                '--http-port', str(ports[i]), '--node-port', str(ports[i+3]),
                '--peers', ','.join(f'127.0.0.1:{ports[j]}' for j in range(3) if j != i),
                '--data-dir', str(Path(directory) / str(i)), '--replication-factor', '3',
                '--snapshot-interval-secs', '1', '--gossip-interval-secs', '1',
                '--heartbeat-interval-secs', '1'], stdout=log, stderr=log)
            eventually(lambda: proc.poll() is None and request(ports[i], '/health')[0] == 200)
            return proc
        try:
            for i in range(3):
                processes.append(start(i))
            for sender in range(3):
                for i in range(3):
                    assert request(ports[sender], f'/v1/cache/key-{i}?ttl=120', 'PUT', f'value-{i}'.encode())[0] == 200
                    expected = json.loads(request(ports[sender], f'/v1/cache/key-{i}')[1])
                    eventually(lambda: all(
                        (copy := json.loads(request(port, '/internal/get', 'POST', {'key': f'key-{i}'})[1]))['found']
                        and copy['expires_at_unix'] == expected['expires_at_unix']
                        and copy['created_at_unix'] == expected['created_at_unix']
                        for port in ports[:3]))
            for i in range(3):
                for port in ports[:3]:
                    status, body = request(port, f'/v1/cache/key-{i}')
                    assert status == 200 and json.loads(body)['value'] == f'value-{i}'
            # Values above Axum's default JSON limit must also work when forwarded.
            payload = b'x' * (768 * 1024)  # JSON byte arrays exceed 2 MiB on the wire.
            assert request(ports[0], '/v1/cache/large?ttl=120', 'PUT', payload)[0] == 200
            for port in ports[:3]:
                status, body = request(port, '/v1/cache/large')
                assert status == 200 and len(json.loads(body)['value']) == len(payload)
            # Read each physical store without routing; all three must contain each key.
            for i in range(3):
                eventually(lambda: all(json.loads(request(p, '/internal/get', 'POST', {'key': f'key-{i}'})[1])['found'] for p in ports[:3]))
                copies = [json.loads(request(p, '/internal/get', 'POST', {'key': f'key-{i}'})[1]) for p in ports[:3]]
                assert len({v['expires_at_unix'] for v in copies}) == 1
                assert len({v['created_at_unix'] for v in copies}) == 1
            eventually(lambda: all(len(json.loads(request(p, '/v1/cluster/nodes')[1])['nodes']) == 3 for p in ports[:3]))
            assert request(ports[1], '/v1/cache/key-0', 'DELETE')[0] == 200
            eventually(lambda: all(not json.loads(request(p, '/internal/get', 'POST', {'key': 'key-0'})[1])['found'] for p in ports[:3]))
            assert request(ports[2], '/v1/cache/short?ttl=1', 'PUT', b'expiring')[0] == 200
            eventually(lambda: all(request(p, '/v1/cache/short')[0] == 404 for p in ports[:3]))
            # Wait for actual snapshot artifacts, then kill abruptly and recover.
            eventually(lambda: all((Path(directory) / str(i) / 'snapshot-current.bin').exists() for i in range(3)))
            for proc in processes:
                proc.kill()
                proc.wait(timeout=5)
            processes.clear()
            for i in range(3):
                processes.append(start(i))
            for port in ports[:3]:
                for i in (1, 2):
                    status, body = request(port, f'/v1/cache/key-{i}')
                    assert status == 200 and json.loads(body)['value'] == f'value-{i}'
                assert request(port, '/v1/cache/key-0')[0] == 404
                assert request(port, '/v1/cache/short')[0] == 404
            print('PASS: 3-node routing, gossip, replication, deletion, TTL, snapshots and crash recovery')
        except Exception:
            for log in logs:
                log.flush()
            for path in Path(directory).glob('*.log'):
                print(path.name, path.read_text()[-5000:])
            raise
        finally:
            for proc in processes:
                if proc.poll() is None:
                    proc.terminate()
                proc.wait(timeout=5)
            for log in logs:
                log.close()

if __name__ == '__main__':
    main()
