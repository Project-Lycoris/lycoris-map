#!/usr/bin/env python3
"""Restore a verified snapshot to a new isolated database and media directory."""
import argparse
import hashlib
import re
import json
import os
from pathlib import Path
import subprocess
import tarfile

if os.geteuid() != 0:
    raise SystemExit('Root required.')
os.umask(0o077)
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--backup', type=Path, required=True)
parser.add_argument('--database', required=True)
parser.add_argument('--uploads', type=Path, required=True)
parser.add_argument('--media-source', type=Path,
                    help='Reuse verified originals already staged locally instead of extracting the archive.')
args = parser.parse_args()
backup, database, uploads = args.backup.resolve(), args.database, args.uploads.resolve()
if not __debug__ or not re.fullmatch(r'lycoris_import_[0-9]+', database):
    raise SystemExit('Use a new explicitly named import database and Python without -O.')
if not backup.is_relative_to('/opt/lycoris/backups') or not uploads.is_relative_to('/opt/lycoris/data'):
    raise SystemExit('Unexpected private backup or media destination.')
container = 'lycoris-shanghai-postgres-1'
project = subprocess.check_output(['docker', 'inspect', '--format',
    '{{index .Config.Labels "com.docker.compose.project"}}', container], text=True).strip()
assert project == 'lycoris-shanghai'
psql = ['docker', 'exec', '-i', container, 'psql', '-XqAt', '-U', 'postgres',
        '-d', 'postgres', '-v', 'ON_ERROR_STOP=1']
exists = subprocess.check_output(psql, input=f"SELECT count(*) FROM pg_database WHERE datname='{database}';\n".encode())
assert int(exists) == 0, 'Target database already exists; refusing overwrite.'
assert not uploads.exists(), 'Target uploads directory already exists.'
if args.media_source:
    media_source = args.media_source.resolve()
    assert media_source.is_relative_to('/opt/lycoris/data')
    # The complete archive may still be transferring. Verify every other backup
    # file and each immutable original before linking it into the new release.
    checksums = ''.join(line + '\n' for line in (backup / 'SHA256SUMS').read_text().splitlines()
                        if line.split('  ', 1)[1] != 'uploads.tar.gz')
    subprocess.run(['sha256sum', '-c', '-'], input=checksums.encode(), cwd=backup, check=True)
else:
    subprocess.run(['sha256sum', '-c', 'SHA256SUMS'], cwd=backup, check=True)
expected = json.loads((backup / 'media.json').read_text())
for name, info in expected.items():
    relative = Path(name)
    assert not relative.is_absolute() and '..' not in relative.parts
    assert len(relative.parts) == 2 and relative.parts[0] in ('avatars', 'markers')
    if args.media_source:
        source = media_source / relative
        assert not source.is_symlink() and not source.parent.is_symlink() and source.is_file()
        assert source.stat().st_size == info['size']
        with source.open('rb') as handle:
            assert hashlib.file_digest(handle, 'sha256').hexdigest() == info['sha256']
subprocess.run(psql, input=(f'CREATE DATABASE {database} OWNER lycoris TEMPLATE template0 '
    "ENCODING 'UTF8' LC_COLLATE 'en_US.utf8' LC_CTYPE 'en_US.utf8';\n").encode(), check=True)
with (backup / 'database.dump').open('rb') as source, (backup / 'restore.stderr').open('wb') as errors:
    subprocess.run(['docker', 'exec', '-i', container, 'pg_restore', '-U', 'postgres',
                    '-d', database, '--no-acl', '--exit-on-error', '--single-transaction'],
                   stdin=source, stderr=errors, check=True)
uploads.mkdir(mode=0o750)
os.chown(uploads, 10001, 10001)
uploads.chmod(0o750)
if args.media_source:
    for name in expected:
        destination = uploads / name
        destination.parent.mkdir(exist_ok=True)
        os.link(media_source / name, destination)
else:
    with tarfile.open(backup / 'uploads.tar.gz', 'r:gz') as archive:
        members = archive.getmembers()
        assert len(members) == len(expected)
        assert {member.name for member in members} == set(expected)
        for member in members:
            assert member.isfile() and member.size == expected[member.name]['size']
        archive.extractall(uploads, members=members, filter='data')
for path in uploads.rglob('*'):
    assert not path.is_symlink()
    os.chown(path, 10001, 10001)
    path.chmod(0o750 if path.is_dir() else 0o640)
print(json.dumps({'database': database, 'originalFilesRestored': len(expected),
                  'applicationSwitched': False}))
