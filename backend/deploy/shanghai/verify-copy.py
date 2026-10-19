#!/usr/bin/env python3
"""Verify restored rows, identity sequences, owners and originals without writes."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from urllib.parse import unquote, urlsplit


SETTINGS = "SET TIME ZONE 'UTC'; SET datestyle='ISO,YMD'; SET bytea_output='hex'; SET extra_float_digits=3;"


def main():
    if not __debug__:
        raise SystemExit('Verification requires Python without optimization (-O).')
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--backup', type=Path, required=True)
    parser.add_argument('--database', required=True)
    parser.add_argument('--uploads', type=Path, required=True)
    args = parser.parse_args()
    if os.geteuid() != 0 or not re.fullmatch(r'lycoris_import_[0-9]+', args.database):
        raise SystemExit('Use root and an explicitly named Shanghai import database.')
    container = 'lycoris-shanghai-postgres-1'
    project = subprocess.check_output([
        'docker', 'inspect', '--format', '{{index .Config.Labels "com.docker.compose.project"}}',
        container,
    ], text=True).strip()
    if project != 'lycoris-shanghai':
        raise SystemExit('Unexpected target Compose project.')
    psql = ['docker', 'exec', '-i', container, 'psql', '-XqAt', '-U', 'postgres', '-d',
            args.database, '-v', 'ON_ERROR_STOP=1']

    def query(sql):
        return subprocess.check_output(psql, input=(SETTINGS + sql + ';\n').encode())

    expected = json.loads((args.backup / 'tables.json').read_text())
    actual_tables = query("SELECT tablename FROM pg_tables WHERE schemaname='public' ORDER BY tablename").decode().splitlines()
    assert set(actual_tables) == set(expected), 'Restored table inventory differs.'
    for table in actual_tables:
        assert re.fullmatch(r'[a-z_][a-z0-9_]*', table), 'Unexpected table identifier.'
        data = query(f'COPY (SELECT row_to_json(t) FROM public."{table}" t '
                     'ORDER BY row_to_json(t)::text COLLATE "C") TO STDOUT')
        actual = {'rows': len(data.splitlines()), 'sha256': hashlib.sha256(data).hexdigest()}
        assert actual == expected[table], f'Row fingerprint mismatch: {table}'
        owner = query(f"SELECT tableowner FROM pg_tables WHERE schemaname='public' AND tablename='{table}'").decode().strip()
        assert owner == ('postgres' if table == 'spatial_ref_sys' else 'lycoris'), f'Owner mismatch: {table}'

    migrations = json.loads(query("SELECT json_agg(t ORDER BY version) FROM "
                                  "(SELECT version,encode(checksum,'hex') AS checksum,success FROM _sqlx_migrations) t"))
    assert migrations == json.loads((args.backup / 'migrations.json').read_text()), 'Migration history mismatch.'
    assert all(item['success'] for item in migrations), 'Incomplete migration found.'

    sequences = {}
    for table in ['users', 'map_markers', 'map_marker_translations', 'marker_favorites',
                  'marker_edit_proposals', 'marker_image_proposals', 'marker_photos']:
        sequence = query(f"SELECT pg_get_serial_sequence('public.{table}','id')").decode().strip()
        assert re.fullmatch(r'public\.[a-z_][a-z0-9_]*', sequence), 'Unexpected identity sequence.'
        sequence_owner = query(f"SELECT pg_get_userbyid(relowner) FROM pg_class WHERE oid='{sequence}'::regclass").decode().strip()
        assert sequence_owner == 'lycoris', f'Sequence owner mismatch: {table}'
        dependency = query(f"SELECT count(*) FROM pg_depend d JOIN pg_attribute a "
                           f"ON a.attrelid=d.refobjid AND a.attnum=d.refobjsubid "
                           f"WHERE d.classid='pg_class'::regclass AND d.objid='{sequence}'::regclass "
                           f"AND d.refclassid='pg_class'::regclass AND d.refobjid='public.{table}'::regclass "
                           "AND a.attname='id' AND d.deptype IN ('i','a')")
        assert int(dependency) == 1, f'Identity dependency mismatch: {table}'
        maximum = int(query(f'SELECT coalesce(max(id),0) FROM public."{table}"'))
        last, called = query(f'SELECT last_value,is_called FROM {sequence}').decode().strip().split('|')
        increment = int(query(f"SELECT seqincrement FROM pg_sequence WHERE seqrelid='{sequence}'::regclass"))
        next_candidate = int(last) + (increment if called == 't' else 0)
        assert increment > 0 and next_candidate > maximum, f'Identity collision risk: {table}'
        sequences[table] = {'maxId': maximum, 'nextCandidate': next_candidate}
    invalid_indexes = int(query("SELECT count(*) FROM pg_index i JOIN pg_class c ON c.oid=i.indexrelid "
                                "JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='public' AND NOT i.indisvalid"))
    assert invalid_indexes == 0, 'Invalid index found.'
    unvalidated_constraints = int(query("SELECT count(*) FROM pg_constraint c JOIN pg_namespace n "
                                       "ON n.oid=c.connamespace WHERE n.nspname='public' AND NOT c.convalidated"))
    assert unvalidated_constraints == 0, 'Unvalidated constraint found.'

    expected_media = json.loads((args.backup / 'media.json').read_text())
    actual_media = {}
    for path in sorted(args.uploads.rglob('*')):
        assert not path.is_symlink(), 'Unexpected restored media symlink.'
        if path.is_file():
            with path.open('rb') as handle:
                digest = hashlib.file_digest(handle, 'sha256').hexdigest()
            actual_media[str(path.relative_to(args.uploads))] = {'size': path.stat().st_size, 'sha256': digest}
    assert actual_media == expected_media, 'Restored media inventory/hash mismatch.'
    references = json.loads((args.backup / 'media-references.json').read_text())
    missing, external = [], []
    for url in references:
        parsed = urlsplit(url)
        path = unquote(parsed.path)
        if parsed.scheme or parsed.netloc or not re.fullmatch(r'/uploads/(avatars|markers)/[^/]+', path) or '..' in Path(path).parts:
            external.append(url)
        elif path.removeprefix('/uploads/') not in actual_media:
            missing.append(url)
    baseline = json.loads((args.backup / 'summary.json').read_text())
    assert missing == baseline['preexistingMissingReferences'], 'New missing media reference.'
    assert external == baseline['externalOrUnsupportedReferences'], 'External reference classification differs.'
    result = {'database': args.database, 'tables': {name: value['rows'] for name,value in expected.items()},
              'originalFiles': len(actual_media), 'missingReferences': len(missing),
              'externalReferences': len(external), 'sequences': sequences, 'invalidIndexes': invalid_indexes,
              'unvalidatedConstraints': unvalidated_constraints}
    print(json.dumps(result))


if __name__ == '__main__':
    main()
