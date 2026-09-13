#!/usr/bin/env python3
"""Real-release capture and immutable offline migration replay. Invoked by make."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
import os
import re
from pathlib import Path
import shutil
import sqlite3
import subprocess
import sys
import tarfile
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
RELEASE = '37a0ea9530a26d8b3e965db09fafc119441dee38'
BUILD = ROOT / 'target' / 'migration-compat'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def checked(argv, **kwargs):
    return subprocess.run([str(a) for a in argv], check=True, **kwargs)


def build_old():
    checkout = BUILD / 'v3.0.0'
    archive = subprocess.check_output(['git', 'archive', RELEASE], cwd=ROOT)
    with tarfile.open(fileobj=io.BytesIO(archive)) as contents:
        if not checkout.exists():
            checkout.mkdir(parents=True)
            contents.extractall(checkout, filter='data')
        for member in contents.getmembers():
            if member.isfile():
                expected = contents.extractfile(member).read()
                if (checkout / member.name).read_bytes() != expected:
                    raise RuntimeError(f'released source was modified: {member.name}')
    examples = checkout / 'zcash_voting' / 'examples'
    (examples / 'migration_history_impl.rs').unlink(missing_ok=True)
    (examples / 'migration_history_impl').mkdir(exist_ok=True)
    shutil.copyfile(ROOT / 'migration-compat/history.rs', examples / 'migration_history_impl/mod.rs')
    (examples / 'migration_history.rs').write_text('#[path = "migration_history_impl/mod.rs"] mod history; fn main() -> anyhow::Result<()> { history::run() }\n')
    shutil.copytree(ROOT / 'migration-compat/old', examples / 'migration_capture', dirs_exist_ok=True)
    (examples / 'migration_capture.rs').write_text('#[path = "migration_capture/mod.rs"] mod capture; fn main() -> anyhow::Result<()> { capture::run() }\n')
    lock_hash = digest(checkout / 'Cargo.lock')
    checked(['cargo', 'build', '--locked', '--release', '-p', 'zcash_voting', '--example', 'migration_history', '--example', 'migration_capture'],
            cwd=checkout, env=dict(os.environ, CARGO_TARGET_DIR=str(BUILD / 'old-build')))
    assert digest(checkout / 'Cargo.lock') == lock_hash, 'old lockfile changed'
    return BUILD / 'old-build/release/examples'


def build_main():
    checked(['cargo', 'build', '--locked', '-p', 'zcash_voting', '--example', 'migration_history'], cwd=ROOT,
            env=dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target/zakura')))
    return ROOT / 'target/zakura/debug/examples/migration_history'


def connect(path):
    return sqlite3.connect(Path(path).resolve().as_uri() + '?mode=ro', uri=True)


def copy_database(source, target):
    with connect(source) as old, sqlite3.connect(target) as new:
        old.backup(new)


def snapshot(path):
    """Exact values, keyed by table and original columns, including BLOBs."""
    with connect(path) as db:
        tables = [r[0] for r in db.execute("SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")]
        return {table: ([r[1] for r in db.execute(f'PRAGMA table_info("{table}")')],
                        list(db.execute(f'SELECT * FROM "{table}" ORDER BY rowid'))) for table in tables}


def assert_preserved(before, after):
    for table, (columns, rows) in before.items():
        if table == 'imt_proofs':
            continue
        new_columns, new_rows = after[table]
        assert len(rows) == len(new_rows), f'{table}: row count changed'
        for row, new_row in zip(rows, new_rows):
            for column, value in zip(columns, row):
                actual = new_row[new_columns.index(column)]
                if table == 'votes' and column == 'commitment_bundle_json' and value is not None:
                    expected = json.loads(value)
                    if expected.get('format') == 'zcash_voting_vote_recovery_v1' and not any(k in expected for k in ('batch_digest', 'batch_index', 'batch_size')):
                        expected.update(batch_digest=None, batch_index=None, batch_size=None)
                    assert json.loads(actual) == expected, 'recovery semantics changed'
                else:
                    assert actual == value, f'{table}.{column} changed'
    if 'imt_proofs' in before:
        cols, rounds = before['rounds']
        networks = {(r[cols.index('round_id')], r[cols.index('wallet_id')]): r[cols.index('network')] for r in rounds}
        cols, proofs = before['imt_proofs']
        cache_cols, cache = after['pir_proof_cache']
        cached = [dict(zip(cache_cols, r)) for r in cache]
        for row in proofs:
            proof = dict(zip(cols, row))
            matches = [r for r in cached if r['wallet_id'] == proof['wallet_id'] and r['network'] == networks[(proof['round_id'], proof['wallet_id'])]
                       and r['root'] == proof['root'] and r['nullifier'] == proof['nullifier']]
            assert len(matches) == 1, 'PIR proof missing or duplicated'
            assert all(matches[0][k] == proof[k] for k in ('nf_bounds', 'leaf_pos', 'path')), 'PIR proof changed'


def read_history(binary, wallet, manifest):
    # The reader has no signing environment. It only calls synchronous storage/planning APIs.
    env = {k: os.environ[k] for k in ('PATH', 'HOME', 'TMPDIR') if k in os.environ}
    command = [binary, wallet, manifest['wallet_id'], manifest['round_id'], ','.join(map(str, manifest['proposals']))]
    if sys.platform == 'darwin' and shutil.which('sandbox-exec'):
        command = ['sandbox-exec', '-p', '(version 1)(allow default)(deny network*)', *command]
    result = checked(command,
                     env=env, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def schema(path):
    def normalize(sql):
        # Keep quoted string bytes; whitespace outside them is not schema semantics.
        parts = re.split(r"('(?:[^']|'')*')", sql)
        return ''.join(part if i % 2 else re.sub(r'\s+', '', part) for i, part in enumerate(parts))
    with connect(path) as db:
        return [(kind, name, normalize(sql)) for kind, name, sql in db.execute(
            "SELECT type,name,sql FROM sqlite_schema WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type,name")]


def replay(directory, old_binary, main_binary):
    manifest = json.loads((directory / 'manifest.json').read_text())
    assert manifest['release_commit'] == RELEASE
    assert manifest['provenance'] == 'live-v3.0.0', 'not a real release capture'
    assert manifest['schema_version'] == 13, 'capture must originate at released schema 13'
    assert manifest['lockfile_sha256'] == digest(BUILD / 'v3.0.0/Cargo.lock'), 'released lockfile identity differs'
    assert digest(directory / 'config.json') == manifest['config_sha256'], 'capture configuration changed'
    assert digest(directory / 'evidence.json') == manifest['evidence_sha256'], 'capture evidence changed'
    for name, checksum in manifest['http_evidence_sha256'].items():
        assert Path(name).name == name, 'HTTP evidence must be a file basename'
        assert digest(directory / 'http-evidence' / name) == checksum, 'HTTP evidence changed'
    assert {c['database'] for c in manifest['captures']} == {'accepted.sqlite', 'confirmed.sqlite'}, 'both capture phases are required'
    assert len(manifest['captures']) == 2, 'capture phases must be unique'
    reports = []
    for capture in manifest['captures']:
        assert Path(capture['database']).name == capture['database'], 'capture must be a file in the artifact directory'
        source = directory / capture['database']
        assert digest(source) == capture['sha256'], 'capture checksum mismatch'
        with connect(source) as db:
            assert db.execute('PRAGMA user_version').fetchone()[0] == 13, 'source is not a released database'
        before = snapshot(source)
        with tempfile.TemporaryDirectory(prefix='migration-replay-') as temp:
            wallet = Path(temp) / 'wallet.sqlite'
            sidecar = Path(str(wallet) + '.voting')
            copy_database(source, sidecar)
            baseline = read_history(old_binary, wallet, manifest)
            assert baseline == capture['history'], 'old release no longer agrees with baseline'
            assert baseline['completed_for_display'], 'source never displayed a completed vote'
            assert {i['proposal_id']: i['choice'] for i in baseline['intents']} == {1: 0, 2: 1, 3: None}, 'capture ballot does not match the realism profile'
            assert {c['proposal_id']: c['choice'] for c in baseline['completed_vote_display']['choices']} == {1: 0, 2: 1, 3: None}, 'completed display lost the expected choices'
            with ThreadPoolExecutor(max_workers=6) as openers:
                histories = list(openers.map(lambda _: read_history(main_binary, wallet, manifest), range(6)))
            assert all(history == baseline for history in histories), 'SDK history changed during concurrent migration opens'
            after = snapshot(sidecar)
            assert_preserved(before, after)
            assert read_history(main_binary, wallet, manifest) == baseline, 'reopened history changed'
            assert snapshot(sidecar) == after, 'history read mutated durable rows'
            with connect(sidecar) as db:
                assert db.execute('PRAGMA integrity_check').fetchall() == [('ok',)]
                assert db.execute('PRAGMA foreign_key_check').fetchall() == []
                version = db.execute('PRAGMA user_version').fetchone()[0]
            fresh_wallet = Path(temp) / 'fresh.sqlite'
            checked([main_binary, '--fresh', fresh_wallet], stdout=subprocess.PIPE)
            assert schema(sidecar) == schema(Path(str(fresh_wallet) + '.voting')), 'migrated schema differs from fresh schema'
            params = Path(temp) / 'round-params.json'
            params.write_text(json.dumps(manifest['round_params']))
            checked([main_binary, '--new-round', wallet, manifest['wallet_id'], params], stdout=subprocess.PIPE)
            extended = read_history(main_binary, wallet, manifest)
            extended['rounds'] = [r for r in extended['rounds'] if r['round_id'] != '0f' * 32]
            assert extended == baseline, 'creating another round changed old history'
            reports.append({'capture': capture['database'], 'schema': version, 'history_equal': True, 'rows_preserved': True})
        checked(['make', 'migration-compat-faults', f'FIXTURE_DB={source}'], cwd=ROOT)
        assert digest(source) == capture['sha256'], 'original capture changed'
    assert reports, 'empty capture set'
    report = {'release_commit': RELEASE, 'main_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
              'main_reader_sha256': digest(main_binary), 'captures': reports, 'aggregate_tally': 'external dependency, not tested'}
    (directory / 'replay-report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps(report, indent=2))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('mode', choices=['build', 'capture', 'replay'])
    parser.add_argument('--fixture-dir', type=Path)
    parser.add_argument('--config', default='')
    args = parser.parse_args()
    os.umask(0o077)
    BUILD.mkdir(parents=True, exist_ok=True)
    old = build_old()
    current = build_main()
    if args.mode == 'capture':
        if not args.config:
            checked([old / 'migration_capture', 'preflight'])
            args.config = str(BUILD / f'capture-{time.time_ns()}.json')
            artifact = BUILD / f'capture-{time.time_ns()}'
            checked(['make', 'migration-compat-provision', f'ARTIFACT_DIR={artifact}', f'CAPTURE_CONFIG={args.config}'], cwd=ROOT)
        config_path = Path(args.config).resolve()
        config = json.loads(config_path.read_text())
        directory = Path(config['artifact_dir']).resolve()
        mnemonic = os.environ.get('VOTE_SDK_VOTER_TEST', '').strip()
        if len(mnemonic.split()) != 24:
            raise RuntimeError('VOTE_SDK_VOTER_TEST must contain the staging test mnemonic')
        seed = hashlib.pbkdf2_hmac('sha512', mnemonic.encode(), b'mnemonic', 2048).hex()
        try:
            checked([old / 'migration_capture', config_path], env=dict(os.environ, MIGRATION_VOTER_SEED=seed))
        except subprocess.CalledProcessError:
            if directory.exists():
                (directory / 'capture-status.json').write_text(json.dumps({'completed': False, 'release_commit': RELEASE, 'producer_sha256': digest(old / 'migration_capture'), 'config_sha256': digest(config_path), 'reason': 'producer failed; inspect retained HTTP responses and sidecar; no successful replay claimed'}, indent=2) + '\n')
            raise
        shutil.copyfile(config_path, directory / 'config.json')
        manifest = json.loads((directory / 'capture.json').read_text())
        manifest.update(release_commit=RELEASE, provenance='live-v3.0.0', schema_version=13,
                        historical_roster=config['historical_roster'],
                        http_evidence_sha256={p.name: digest(p) for p in sorted((directory / 'http-evidence').glob('*.json'))}, lockfile_sha256=digest(BUILD / 'v3.0.0/Cargo.lock'),
                        producer_sha256=digest(old / 'migration_capture'),
                        profile={'helpers': 1, 'shares_per_vote': 16, 'last_moment_buffer_seconds': 7200},
                        config_sha256=digest(config_path), evidence_sha256=digest(directory / 'evidence.json'))
        for capture in manifest['captures']:
            source = directory / capture['database']
            capture['sha256'] = digest(source)
            with tempfile.TemporaryDirectory(prefix='old-history-') as temp:
                wallet = Path(temp) / 'wallet.sqlite'
                copy_database(source, Path(str(wallet) + '.voting'))
                capture['history'] = read_history(old / 'migration_history', wallet, manifest)
                assert capture['history']['completed_for_display'], 'capture not completed under old SDK'
        (directory / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
        replay(directory, old / 'migration_history', current)
    elif args.mode == 'replay':
        if not args.fixture_dir:
            parser.error('replay requires --fixture-dir')
        replay(args.fixture_dir.resolve(), old / 'migration_history', current)


if __name__ == '__main__':
    if not __debug__:
        raise RuntimeError('validation must not run with Python optimization enabled')
    try:
        main()
    except (subprocess.CalledProcessError, AssertionError, RuntimeError) as error:
        print(f'COMPATIBILITY VALIDATION FAILED: {error}', file=sys.stderr)
        sys.exit(1)
