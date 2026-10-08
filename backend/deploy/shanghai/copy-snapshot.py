#!/usr/bin/env python3
"""Read-only PostgreSQL snapshot plus immutable originals; write a private backup.

Run on the source host as root. No service, database or credential is changed.
All database reads share one exported snapshot. Sequence counters are not MVCC;
the restore must retain the dump's values and verify they exceed restored IDs.
"""

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tarfile
from urllib.parse import unquote, urlsplit


SETTINGS = "SET TIME ZONE 'UTC'; SET datestyle='ISO,YMD'; SET bytea_output='hex'; SET extra_float_digits=3;"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    if os.geteuid() != 0:
        raise SystemExit('Run as root on the source host.')
    container = 'lycoris-production-postgres-1'
    project = subprocess.check_output([
        'docker', 'inspect', '--format', '{{index .Config.Labels "com.docker.compose.project"}}',
        container,
    ], text=True).strip()
    if project != 'lycoris-production':
        raise SystemExit('Unexpected source Compose project.')
    os.umask(0o077)
    backup = args.output.resolve()
    if not backup.is_relative_to(Path('/opt/lycoris/backups')):
        raise SystemExit('Backup must be under the private backup root.')
    backup.mkdir(mode=0o700, parents=False, exist_ok=False)
    psql = ['docker', 'exec', '-i', container, 'psql', '-XqAt', '-U', 'postgres', '-d',
            'lycoris', '-v', 'ON_ERROR_STOP=1']
    holder = subprocess.Popen(psql, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                              stderr=(backup / 'snapshot.stderr').open('wb'))
    try:
        holder.stdin.write(b'BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY;\nSELECT pg_export_snapshot();\n')
        holder.stdin.flush()
        snapshot = holder.stdout.readline().decode().strip()
        if not re.fullmatch(r'[A-Fa-f0-9]+-[A-Fa-f0-9]+-\d+', snapshot):
            raise RuntimeError('Could not export a PostgreSQL snapshot; inspect private log.')
        snapshot_established = datetime.datetime.now(datetime.timezone.utc).isoformat()

        def query(sql):
            request = (f"BEGIN ISOLATION LEVEL REPEATABLE READ READ ONLY; SET TRANSACTION SNAPSHOT '{snapshot}';"
                       + SETTINGS + sql + ';ROLLBACK;\n').encode()
            return subprocess.check_output(psql, input=request)

        tables = query("SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename").decode().splitlines()
        if not {'users', 'map_markers', '_sqlx_migrations', 'marker_image_uploads'} <= set(tables):
            raise RuntimeError('Source schema does not match the expected application.')
        if not all(re.fullmatch(r'[a-z_][a-z0-9_]*', table) for table in tables):
            raise RuntimeError('Unexpected table identifier.')

        fingerprints = {}
        for table in tables:
            data = query(f'COPY (SELECT row_to_json(t) FROM public."{table}" t '
                         'ORDER BY row_to_json(t)::text COLLATE "C") TO STDOUT')
            fingerprints[table] = {'rows': len(data.splitlines()), 'sha256': hashlib.sha256(data).hexdigest()}
        (backup / 'tables.json').write_text(json.dumps(fingerprints, indent=2) + '\n')
        (backup / 'migrations.json').write_bytes(query(
            "SELECT json_agg(t ORDER BY version) FROM "
            "(SELECT version,encode(checksum,'hex') AS checksum,success FROM _sqlx_migrations) t"))
        urls = json.loads(query(
            "SELECT coalesce(json_agg(url ORDER BY url),'[]'::json) FROM ("
            "SELECT avatar_url AS url FROM users UNION SELECT mark_image FROM map_markers "
            "UNION SELECT image_url FROM marker_image_proposals "
            "UNION SELECT image_url FROM marker_photos) r WHERE url IS NOT NULL AND url <> ''"))
        (backup / 'media-references.json').write_text(json.dumps(urls, ensure_ascii=False, indent=2) + '\n')

        with (backup / 'database.dump').open('wb') as output, (backup / 'pg-dump.stderr').open('wb') as errors:
            subprocess.run(['docker', 'exec', container, 'pg_dump', '-U', 'postgres', '-d', 'lycoris',
                            '-Fc', '--snapshot=' + snapshot], stdout=output, stderr=errors, check=True)
        with (backup / 'database.dump').open('rb') as data, (backup / 'contents.txt').open('wb') as output:
            subprocess.run(['docker', 'exec', '-i', container, 'pg_restore', '--list'],
                           stdin=data, stdout=output, check=True)
        holder.stdin.write(b'ROLLBACK;\n\\q\n')
        holder.stdin.flush()
        holder.communicate(timeout=15)
        if holder.returncode != 0:
            raise RuntimeError('Snapshot holder failed.')
    finally:
        if holder.poll() is None:
            holder.terminate()
            holder.wait(timeout=15)

    # Final images predate the commits referencing them and are never overwritten.
    # Exclude regenerable rendition caches and uncommitted .tmp-* files.
    uploads = Path('/opt/lycoris/data/uploads')
    paths = []
    for directory in ['avatars', 'markers']:
        parent = uploads / directory
        if parent.is_symlink():
            raise RuntimeError('Unexpected media directory symlink.')
        if not parent.exists():
            continue
        for path in sorted(parent.rglob('*')):
            if path.is_symlink():
                raise RuntimeError('Unexpected media symlink.')
            if path.is_file() and not path.name.startswith('.tmp-'):
                paths.append(path)

    manifest = {}
    with tarfile.open(backup / 'uploads.tar.gz', 'w:gz', compresslevel=1) as archive:
        for path in paths:
            relative = str(path.relative_to(uploads))
            before = path.stat()
            with path.open('rb') as handle:
                contents = handle.read()
            after = path.stat()
            if (before.st_size, before.st_mtime_ns, before.st_ino) != (after.st_size, after.st_mtime_ns, after.st_ino):
                raise RuntimeError('A final media file changed during capture; retry with a new backup directory.')
            if len(contents) != before.st_size:
                raise RuntimeError('Incomplete media read.')
            import io
            entry = tarfile.TarInfo(relative)
            entry.size = len(contents)
            entry.mtime = before.st_mtime
            entry.mode = 0o640
            entry.uid = entry.gid = 10001
            archive.addfile(entry, io.BytesIO(contents))
            manifest[relative] = {'size': len(contents), 'sha256': hashlib.sha256(contents).hexdigest()}
    (backup / 'media.json').write_text(json.dumps(manifest, indent=2) + '\n')

    missing, external = [], []
    for url in urls:
        parsed = urlsplit(url)
        path = unquote(parsed.path)
        if parsed.scheme or parsed.netloc or not re.fullmatch(r'/uploads/(avatars|markers)/[^/]+', path) or '..' in Path(path).parts:
            external.append(url)
        elif path.removeprefix('/uploads/') not in manifest:
            missing.append(url)
    summary = {
        'snapshotEstablishedAtUTC': snapshot_established,
        'backupCompletedAtUTC': datetime.datetime.now(datetime.timezone.utc).isoformat(),
        'snapshot': snapshot,
        'sourceDatabase': 'lycoris',
        'tableRows': {name: info['rows'] for name, info in fingerprints.items()},
        'originalFiles': len(manifest),
        'originalBytes': sum(info['size'] for info in manifest.values()),
        'referencedImages': len(urls),
        'preexistingMissingReferences': missing,
        'externalOrUnsupportedReferences': external,
        'renditionCacheCopied': False,
        'redisOrConfigurationCopied': False,
    }
    (backup / 'summary.json').write_text(json.dumps(summary, ensure_ascii=False, indent=2) + '\n')
    sums = []
    for path in sorted(backup.iterdir()):
        if path.is_file():
            with path.open('rb') as handle:
                digest = hashlib.file_digest(handle, 'sha256').hexdigest()
            sums.append(f'{digest}  {path.name}')
    (backup / 'SHA256SUMS').write_text('\n'.join(sums) + '\n')
    print(json.dumps({'backup': str(backup), 'tables': summary['tableRows'],
                      'originalFiles': len(manifest), 'missingReferences': len(missing),
                      'externalReferences': len(external)}))


if __name__ == '__main__':
    main()
