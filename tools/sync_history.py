"""Maintain a component's own published history, using immutable release sources.

The same dependency-free script is vendored as tools/sync_history.py in Manager.
Historical corrections live in the reviewed history JSON, not in moving branches.
"""
import argparse
import base64
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import re
import subprocess
import tempfile

SERVER_AREAS = {'server', 'bots', 'scaling', 'modules', 'addon', 'client'}
MANAGER_AREAS = {'manager', 'client'}


def gh(*args):
    return subprocess.check_output(['gh', *args], encoding='utf-8')


def api(path):
    return json.loads(gh('api', path))


def version_key(version):
    if not re.fullmatch(r'\d+(?:\.\d+)+', version):
        raise ValueError('Invalid published version: ' + version)
    return tuple(map(int, version.split('.')))


def eligible(release, component):
    pattern = r'v(\d+(?:\.\d+)+)' if component == 'manager' else r'server-(\d+(?:\.\d+)+)'
    return not release['draft'] and re.fullmatch(pattern, release['tag_name'])


def published(repo, component):
    pages = json.loads(gh('api', '--paginate', '--slurp', f'repos/{repo}/releases?per_page=100'))
    return sorted([r for page in pages for r in page if eligible(r, component)],
                  key=lambda r: version_key(r['tag_name'].removeprefix('server-').removeprefix('v')))


def fragment_note(text, areas):
    sections = text.lstrip('\ufeff').strip().split('---', 2)
    if len(sections) != 3 or sections[0]:
        raise ValueError('Invalid release fragment')
    fields = dict(line.split(':', 1) for line in sections[1].splitlines() if ':' in line)
    fields = {k.strip(): v.strip().strip('"\'') for k, v in fields.items()}
    if fields.get('area') not in areas:
        return None
    if not fields.get('title'):
        raise ValueError('Release fragment has no title')
    if re.search(r'\b(compilation|compiler|cache|manual builds|repository owners|packaging time)\b', fields['title'], re.I):
        return None
    # Keep public notes player-facing. Technical refs remain in the source fragment.
    title = fields['title'].rstrip('.')
    return title


def remote_fragments(repo, sha, seen, areas, excluded_blobs=(), overrides=None):
    if not re.fullmatch(r'[0-9a-f]{40}', sha):
        raise ValueError('Changelog collection requires an immutable source SHA')
    listing = api(f'repos/{repo}/contents/changelog.d?ref={sha}')
    files = [f for f in listing if re.fullmatch(r'\d{8}-[a-z0-9-]+\.md', f['name'])]
    new = [f for f in files if repo + ':' + f['name'] not in seen and f['sha'] not in excluded_blobs]

    def read(item):
        reviewed = (overrides or {}).get(repo + ':' + item['sha'])
        if reviewed is not None:
            if not isinstance(reviewed, str) or not reviewed.strip():
                raise ValueError('Reviewed fragment override needs a player-facing title')
            return reviewed
        blob = api(f'repos/{repo}/git/blobs/{item["sha"]}')
        try:
            return fragment_note(base64.b64decode(blob['content']).decode('utf-8'), areas)
        except ValueError as error:
            raise ValueError(f'{repo}:{item["name"]}@{sha}: {error}') from error

    with ThreadPoolExecutor(max_workers=6) as pool:
        notes = [note for note in pool.map(read, new) if note]
    return notes, [repo + ':' + f['name'] for f in files]


def collect_manager(root, tag, seen):
    sha = subprocess.check_output(['git', '-C', str(root), 'rev-parse', tag + '^{commit}'], encoding='utf8').strip()
    paths = subprocess.check_output(['git', '-C', str(root), 'ls-tree', '-r', '--name-only', sha, 'changelog.d'], encoding='utf8').splitlines()
    paths = [p for p in paths if re.fullmatch(r'changelog.d/\d{8}-[a-z0-9-]+\.md', p)]
    repo = 'Corfirean/coa-server-manager'
    notes = []
    for path in paths:
        if repo + ':' + Path(path).name not in seen:
            text = subprocess.check_output(['git', '-C', str(root), 'show', f'{sha}:{path}'], encoding='utf8')
            note = fragment_note(text, MANAGER_AREAS)
            if note:
                notes.append(note)
    source = {'commit': sha}
    if any('portable' in note.lower() and 'experimental' in note.lower() for note in notes):
        source['experimentalFeatures'] = ['portable-characters']
    return notes, [repo + ':' + Path(p).name for p in paths], source


def upstream_fragment_blobs(lock):
    upstream = lock['components'].get('upstream')
    if not upstream:
        return set()
    prefix = 'repos/' + upstream['repository'] + '/git/trees/'
    tree = api(prefix + upstream['sha'])['tree']
    directory = next((item for item in tree if item['path'] == 'changelog.d' and item['type'] == 'tree'), None)
    if not directory:
        return set()
    files = api(prefix + directory['sha'])['tree']
    return {item['sha'] for item in files if item['type'] == 'blob' and re.fullmatch(r'\d{8}-[a-z0-9-]+\.md', item['path'])}


def collect_server(repo, tag, seen, previous, overrides=None):
    with tempfile.TemporaryDirectory() as folder:
        subprocess.run(['gh', 'release', 'download', tag, '--repo', repo, '--pattern', 'release-lock.json', '--dir', folder], check=True)
        lock = json.loads((Path(folder) / 'release-lock.json').read_text(encoding='utf-8-sig'))
    notes, ids = [], []
    imported_blobs = upstream_fragment_blobs(lock)
    for role in ('core', 'bots', 'scaling', 'build'):
        item = lock['components'].get(role)
        if not item:
            raise ValueError('Release lock has no ' + role + ' source')
        if item['repository'] == 'Corfirean/coa-server-manager':
            raise ValueError('A Manager source cannot supply server release notes')
        entries, found = remote_fragments(item['repository'], item['sha'], seen, SERVER_AREAS,
                                         imported_blobs if role == 'core' else (), overrides)
        notes.extend(entries)
        ids.extend(found)
    old = previous.get('components', {})
    # Imported updates are explicitly updates, never attributed as our authored fixes.
    for role, label in [('races', 'Custom Races'), ('squid', 'SQUID Playerbots')]:
        item = lock['components'].get(role)
        if item and item['sha'] != old.get(role, {}).get('sha'):
            ref = item.get('ref', '')
            version = re.sub(r'^(coa-custom-|v)', '', ref)
            if version and not re.fullmatch(r'[0-9a-f]{40}|main|master', version):
                notes.append(f'Updated {label} to {version}')
    if lock['components'].get('upstream', {}).get('sha') != old.get('upstream', {}).get('sha'):
        notes.append('Updated the reviewed CoA upstream snapshot')
    return list(dict.fromkeys(notes)), ids, {'snapshot': lock['snapshot'], 'components': lock['components'],
                                           'experimentalFeatures': lock.get('experimentalFeatures', [])}


def notes_md(record):
    status = ' (Pre-release)' if record.get('prerelease') else ''
    lines = [f'## {record["version"]}{status} — {record["date"]}', '']
    if record.get('notice'):
        lines += [record['notice'], '']
    if 'portable-characters' in record.get('experimentalFeatures', []):
        lines += ['**Portable Characters are experimental and may contain bugs.**', '']
    lines.extend('- ' + note.strip().rstrip('.') + '.' for note in record['notes'])
    label = 'Archived release record' if record.get('legacy') else 'Release'
    lines += ['', f'[{label}]({record["url"]})']
    if record.get('compare'):
        lines[-1] += f' · [Source changes]({record["compare"]})'
    if record.get('links'):
        lines += ['', *(link.strip() for link in record['links'])]
    return '\n'.join(lines) + '\n'


def render(history):
    component = history['component']
    title = 'CoA Server Manager' if component == 'manager' else 'CoA Server'
    other = ('[Server changelog](https://github.com/Corfirean/coa-server-build/blob/main/CHANGELOG.md)' if component == 'manager'
             else '[Manager changelog](https://github.com/Corfirean/coa-server-manager/blob/master/CHANGELOG.md)')
    lines = [f'# {title} changelog', '',
             ('Manager releases only. Updating the Manager does not update the server.' if component == 'manager'
              else 'Server packages, companions and server modules only. Manager releases have their own changelog.'),
             '', other, '', 'Published releases and previews. Pre-release entries do not change the stable update channel.', '',
             'Dates follow GitHub publication (UTC); legacy records retain their archived dates. Newest releases first.', '']
    for record in sorted(history['releases'], key=lambda r: (r['date'], version_key(r['version'])), reverse=True):
        lines.append(notes_md(record))
    return '\n'.join(lines).rstrip() + '\n'


def validate_history(history):
    expected = {'server': 'Corfirean/coa-server-build', 'manager': 'Corfirean/coa-server-manager'}
    component = history.get('component')
    if history.get('schema') != 1 or history.get('repository') != expected.get(component):
        raise ValueError('History must belong to exactly one known component repository')
    versions, tags = set(), set()
    for record in history['releases']:
        version_key(record['version'])
        if record['version'] in versions or record['tag'] in tags:
            raise ValueError('Duplicate release in component history')
        versions.add(record['version']); tags.add(record['tag'])
        prefix = 'v' if component == 'manager' else 'server-'
        if not record.get('legacy') and record['tag'] != prefix + record['version']:
            raise ValueError('Release tag belongs to a different component/version')
        if not record.get('notes') or not all(isinstance(n, str) and n.strip() for n in record['notes']):
            raise ValueError('Release notes must contain confirmed changes')
    if component == 'server' and any(i.startswith('Corfirean/coa-server-manager:') for i in history.get('consumedFragments', [])):
        raise ValueError('Server history cannot consume Manager fragments')


def reconcile(history, releases, root):
    component, repo = history['component'], history['repository']
    records = {r['tag']: r for r in history['releases']}
    seen = set(history.get('consumedFragments', []))
    for release in releases:
        tag = release['tag_name']
        preview = bool(release.get('prerelease'))
        if tag in records:
            record = records[tag]
            if preview:
                record['prerelease'] = True
            elif record.pop('prerelease', False):
                seen.update(record.pop('fragments', []))
            continue
        version = tag.removeprefix('server-').removeprefix('v')
        older = [r for r in history['releases'] if not r.get('prerelease')
                 and version_key(r['version']) < version_key(version)]
        previous = max(older, key=lambda r: version_key(r['version']), default={})
        if component == 'manager':
            notes, ids, source = collect_manager(root, tag, frozenset(seen))
        else:
            notes, ids, source = collect_server(repo, tag, frozenset(seen), previous,
                                               history.get('fragmentOverrides'))
        if not notes:
            notes = ['No additional player-facing changes were recorded for this build']
        record = {'tag': tag, 'version': version, 'date': release['published_at'][:10],
                  'url': release['html_url'], 'notes': notes, **source}
        if preview:
            record.update(prerelease=True, fragments=ids)
        else:
            seen.update(ids)
        if previous.get('tag'):
            record['compare'] = f'https://github.com/{repo}/compare/{previous["tag"]}...{tag}'
        history['releases'].append(record)
        records[tag] = record
    history['consumedFragments'] = sorted(seen)
    validate_history(history)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--root', type=Path, default=Path.cwd())
    parser.add_argument('--history', default='changelog/history.json')
    parser.add_argument('--sync-release-notes', action='store_true')
    parser.add_argument('--render-only', action='store_true')
    args = parser.parse_args()
    path = args.root / args.history
    history = json.loads(path.read_text(encoding='utf8'))
    validate_history(history)
    component, repo = history['component'], history['repository']
    if component not in ('server', 'manager'):
        raise ValueError('History must belong to one component')
    releases = [] if args.render_only else published(repo, component)
    reconcile(history, releases, args.root)
    # Keep a separately reviewed history as the source of truth for historical corrections.
    path.write_text(json.dumps(history, indent=2, ensure_ascii=False) + '\n', encoding='utf8', newline='\n')
    (args.root / 'CHANGELOG.md').write_text(render(history), encoding='utf8', newline='\n')
    if args.sync_release_notes:
        actual = {r['tag_name']: r for r in (releases or published(repo, component))}
        for record in history['releases']:
            release = actual.get(record['tag'])
            if not release:
                continue  # Never create a missing historical release.
            body = notes_md(record)
            if (release.get('body') or '').strip() != body.strip():
                with tempfile.TemporaryDirectory() as folder:
                    note_file = Path(folder) / 'notes.md'
                    note_file.write_text(body, encoding='utf8', newline='\n')
                    subprocess.run(['gh', 'release', 'edit', record['tag'], '--repo', repo, '--notes-file', str(note_file)], check=True)
    print(f'{component}: {len(history["releases"])} releases; component changelog synchronized')


if __name__ == '__main__':
    main()
