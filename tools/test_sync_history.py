import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch
import sync_history as sync


class ComponentHistoryTests(unittest.TestCase):
    def history(self):
        return {'schema': 1, 'component': 'manager', 'repository': 'Corfirean/coa-server-manager',
                'consumedFragments': [], 'releases': [{'version': '0.6.22', 'tag': 'v0.6.22',
                'date': '2026-10-19', 'url': 'https://example.test/release', 'notes': ['Existing client files are preserved']}]}

    def test_manager_history_cannot_be_used_as_server_history(self):
        history = self.history()
        history['component'] = 'server'
        with self.assertRaises(ValueError): sync.validate_history(history)

    def test_duplicate_release_and_wrong_tag_are_rejected(self):
        history = self.history()
        history['releases'].append(copy.deepcopy(history['releases'][0]))
        with self.assertRaises(ValueError): sync.validate_history(history)
        history['releases'].pop()
        history['releases'][0]['tag'] = 'server-0.6.22'
        with self.assertRaises(ValueError): sync.validate_history(history)

    def test_published_previews_are_included_but_drafts_and_other_components_are_not(self):
        release = {'tag_name': 'v0.7.0', 'draft': True, 'prerelease': False}
        self.assertFalse(sync.eligible(release, 'manager'))
        release.update(draft=False, prerelease=True)
        self.assertTrue(sync.eligible(release, 'manager'))
        release.update(prerelease=False)
        self.assertTrue(sync.eligible(release, 'manager'))
        self.assertFalse(sync.eligible(release, 'server'))
        self.assertFalse(sync.eligible(dict(release, tag_name='stable'), 'server'))

    def release(self, version='0.7.0', preview=True):
        return {'tag_name': 'v' + version, 'prerelease': preview, 'draft': False,
                'published_at': '2026-10-22T12:00:00Z', 'html_url': 'https://example.test/v' + version}

    def test_preview_does_not_consume_stable_changes_and_repeated_sync_is_idempotent(self):
        history = self.history()
        with patch.object(sync, 'collect_manager', return_value=(['Portable Characters are experimental'], ['own:portable'], {'commit': 'a' * 40})) as collect:
            sync.reconcile(history, [self.release()], Path('.'))
            sync.reconcile(history, [self.release()], Path('.'))
        self.assertEqual(collect.call_count, 1)
        self.assertEqual(history['consumedFragments'], [])
        self.assertEqual(len(history['releases']), 2)
        self.assertTrue(history['releases'][-1]['prerelease'])
        self.assertIn('## 0.7.0 (Pre-release)', sync.render(history))

    def test_promotion_updates_the_same_reviewed_record_without_collecting_or_duplicating(self):
        history = self.history()
        with patch.object(sync, 'collect_manager', return_value=(['Portable Characters are experimental'], ['own:portable'], {'commit': 'a' * 40})):
            sync.reconcile(history, [self.release()], Path('.'))
        history['releases'][-1]['notes'] = ['Reviewed player-facing text']
        with patch.object(sync, 'collect_manager', side_effect=AssertionError('Frozen notes already recorded')):
            sync.reconcile(history, [self.release(preview=False)], Path('.'))
            sync.reconcile(history, [self.release(preview=False)], Path('.'))
        self.assertEqual(len(history['releases']), 2)
        self.assertNotIn('prerelease', history['releases'][-1])
        self.assertEqual(history['consumedFragments'], ['own:portable'])
        self.assertEqual(history['releases'][-1]['notes'], ['Reviewed player-facing text'])

    def test_stable_hotfix_after_higher_preview_keeps_its_own_notes(self):
        history = self.history()
        with patch.object(sync, 'collect_manager', return_value=(['Shared improvement'], ['own:shared'], {'commit': 'a' * 40})):
            sync.reconcile(history, [self.release()], Path('.'))
        with patch.object(sync, 'collect_manager', return_value=(['Shared improvement'], ['own:shared'], {'commit': 'b' * 40})) as collect:
            sync.reconcile(history, [self.release('0.6.23', preview=False), self.release()], Path('.'))
        self.assertEqual(collect.call_args.args[2], set())
        hotfix = next(r for r in history['releases'] if r['version'] == '0.6.23')
        self.assertEqual(hotfix['notes'], ['Shared improvement'])
        self.assertEqual(history['consumedFragments'], ['own:shared'])

    def test_server_preview_uses_only_server_locked_sources_and_keeps_notice(self):
        history = {'schema': 1, 'component': 'server', 'repository': 'Corfirean/coa-server-build',
                   'consumedFragments': [], 'releases': []}
        release = dict(self.release(), tag_name='server-0.261022.75')
        with patch.object(sync, 'collect_server', return_value=(['Experimental Portable Characters support'], ['core:portable'], {'snapshot': 'a' * 64})), \
             patch.object(sync, 'collect_manager', side_effect=AssertionError('Wrong component')):
            sync.reconcile(history, [release], Path('.'))
        history['releases'][0]['notice'] = 'Portable Characters are experimental.'
        text = sync.render(history)
        self.assertIn('## 0.261022.75 (Pre-release)', text)
        self.assertIn('Portable Characters are experimental.', text)
        self.assertEqual(history['consumedFragments'], [])

    def test_fragment_classification_preserves_source_boundaries(self):
        text = '---\narea: manager\ntitle: Updates recover safely\n---\nYour files are preserved.'
        self.assertIsNone(sync.fragment_note(text, sync.SERVER_AREAS))
        self.assertIn('Updates recover safely', sync.fragment_note(text, sync.MANAGER_AREAS))

    def test_remote_collection_uses_frozen_sha_and_reads_only_new_fragments(self):
        with patch.object(sync, 'api', return_value=[{'name': '20261019-a.md', 'sha': 'b' * 40}]) as api:
            notes, ids = sync.remote_fragments('owner/repo', 'a' * 40, {'owner/repo:20261019-a.md'}, sync.SERVER_AREAS)
            self.assertEqual(notes, [])
            self.assertEqual(len(ids), 1)
            api.assert_called_once_with('repos/owner/repo/contents/changelog.d?ref=' + 'a' * 40)
        with self.assertRaises(ValueError): sync.remote_fragments('owner/repo', 'main', set(), sync.SERVER_AREAS)

    def test_manager_collection_reads_the_real_frozen_tag(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            fragments = root / 'changelog.d'
            fragments.mkdir()
            own = fragments / '20261021-own.md'
            own.write_text('---\narea: manager\ntitle: Characters show their saved versions\n---\n', encoding='utf8')
            (fragments / '20261021-server.md').write_text('---\narea: server\ntitle: Server only\n---\n', encoding='utf8')
            subprocess.run(['git', 'init', '-q', str(root)], check=True)
            subprocess.run(['git', '-C', str(root), 'add', '.'], check=True)
            subprocess.run(['git', '-C', str(root), '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', '-c', 'commit.gpgsign=false', 'commit', '-qm', 'Frozen notes'], check=True)
            subprocess.run(['git', '-C', str(root), 'tag', 'v0.7.0'], check=True)
            own.write_text('Uncommitted content must not become release notes', encoding='utf8')
            notes, ids, source = sync.collect_manager(root, 'v0.7.0', set())
            self.assertEqual(notes, ['Characters show their saved versions'])
            self.assertEqual(len(ids), 2)
            self.assertEqual(len(source['commit']), 40)
            seen = {'Corfirean/coa-server-manager:20261021-own.md'}
            self.assertEqual(sync.collect_manager(root, 'v0.7.0', seen)[0], [])

    def test_utf8_bom_does_not_break_a_valid_historical_fragment(self):
        text = '\ufeff---\narea: bots\ntitle: Companions keep their combat form\n---\n'
        self.assertEqual(sync.fragment_note(text, sync.SERVER_AREAS), 'Companions keep their combat form')

    def test_imported_upstream_fragments_are_not_credited_as_our_changes(self):
        import base64
        listing = [{'name': '20261021-imported.md', 'sha': 'upstream'}, {'name': '20261021-own.md', 'sha': 'own'}]
        body = base64.b64encode(b'---\narea: server\ntitle: Our portable sessions\n---\n').decode()
        with patch.object(sync, 'api', side_effect=[listing, {'content': body}]) as calls:
            notes, consumed = sync.remote_fragments('owner/core', 'a' * 40, set(), sync.SERVER_AREAS, {'upstream'})
        self.assertEqual(notes, ['Our portable sessions'])
        self.assertEqual(len(consumed), 2)
        self.assertEqual(calls.call_count, 2)
        self.assertTrue(calls.call_args.args[0].endswith('/git/blobs/own'))

    def test_reviewed_legacy_fragment_override_is_bound_to_exact_blob(self):
        import base64
        listing = [{'name': '20261006-legacy.md', 'sha': 'legacy-blob'}]
        with patch.object(sync, 'api', return_value=listing) as calls:
            notes, _ = sync.remote_fragments('owner/core', 'a' * 40, set(), sync.SERVER_AREAS,
                                             overrides={'owner/core:legacy-blob': 'Bots retain their combat form'})
        self.assertEqual(notes, ['Bots retain their combat form'])
        self.assertEqual(calls.call_count, 1)
        with patch.object(sync, 'api', side_effect=[listing, {'content': base64.b64encode(b'Malformed new note').decode()}]):
            with self.assertRaisesRegex(ValueError, 'owner/core:20261006-legacy.md'):
                sync.remote_fragments('owner/core', 'a' * 40, set(), sync.SERVER_AREAS,
                                      overrides={'owner/core:different-blob': 'Must not hide malformed notes'})

    def test_upstream_exclusions_use_the_frozen_source_tree(self):
        lock = {'components': {'upstream': {'repository': 'upstream/core', 'sha': 'frozen'}}}
        with patch.object(sync, 'api', side_effect=[{'tree': [{'path': 'changelog.d', 'type': 'tree', 'sha': 'notes-tree'}]}, {'tree': [{'path': '20261021-imported.md', 'type': 'blob', 'sha': 'imported'}, {'path': 'README.md', 'type': 'blob', 'sha': 'readme'}]}]) as calls:
            self.assertEqual(sync.upstream_fragment_blobs(lock), {'imported'})
        self.assertEqual(calls.call_args_list[0].args[0], 'repos/upstream/core/git/trees/frozen')
        self.assertEqual(calls.call_args_list[1].args[0], 'repos/upstream/core/git/trees/notes-tree')

    def test_render_is_component_scoped_and_stable(self):
        text = sync.render(self.history())
        self.assertIn('# CoA Server Manager changelog', text)
        self.assertEqual(text.count('## 0.6.22'), 1)
        self.assertNotIn('## 0.7.0', text)
        self.assertEqual(text, sync.render(self.history()))

    def test_reviewed_windows_release_text_renders_without_carriage_returns(self):
        record = self.history()['releases'][0]
        record.update(notes=['Existing files are preserved\r'], links=['[Guide](https://example.test/guide)\r'])
        text = sync.notes_md(record)
        self.assertIn('- Existing files are preserved.\n', text)
        self.assertNotIn('\r', text)

    def test_render_only_does_not_contact_github_or_change_published_history(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root / 'changelog').mkdir()
            path = root / 'changelog/history.json'
            path.write_text(json.dumps(self.history()), encoding='utf8')
            with patch.object(sys, 'argv', ['sync', '--root', str(root), '--render-only']), patch.object(sync, 'gh', side_effect=AssertionError('No network')):
                sync.main()
                first = (root / 'CHANGELOG.md').read_bytes()
                sync.main()
                self.assertEqual(first, (root / 'CHANGELOG.md').read_bytes())
                self.assertEqual(len(json.loads(path.read_text())['releases']), 1)

    def test_repeated_sync_adds_a_published_version_only_once(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder); (root / 'changelog').mkdir()
            path = root / 'changelog/history.json'
            path.write_text(json.dumps(self.history()), encoding='utf8')
            release = {'tag_name': 'v0.6.23', 'published_at': '2026-10-20T12:00:00Z', 'html_url': 'https://example.test/new'}
            with patch.object(sys, 'argv', ['sync', '--root', str(root)]), patch.object(sync, 'published', return_value=[release]), patch.object(sync, 'collect_manager', return_value=(['Update checks recover safely'], ['own:fragment.md'], {'commit': 'a' * 40})) as collect:
                sync.main(); sync.main()
                self.assertEqual(collect.call_count, 1)
                history = json.loads(path.read_text())
                self.assertEqual(len(history['releases']), 2)
                self.assertEqual(history['releases'][0]['notes'], self.history()['releases'][0]['notes'])
                self.assertEqual(history['consumedFragments'], ['own:fragment.md'])


if __name__ == '__main__': unittest.main()
