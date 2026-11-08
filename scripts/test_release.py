"""Release safety gates; no credentials, Apple requests or GitHub writes."""
import importlib.util
import json
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release', Path(__file__).with_name('release.py'))
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def test_existing_tag_or_missing_auth_never_passes_publication_preflight(self):
        class FakeRunner:
            def __init__(self, code, error): self.code, self.error = code, error
            def run(self, args, **kwargs): return subprocess.CompletedProcess(args, self.code, '{}', self.error)
        for code, error in [(0, ''), (1, 'HTTP 401'), (1, 'connection failed')]:
            with self.assertRaises(release.ReleaseError): release.require_new_tag('v0.1.5', FakeRunner(code, error))
        release.require_new_tag('v0.1.5', FakeRunner(1, 'HTTP 404'))

    def test_only_final_signer_receives_updater_credentials(self):
        env = {'TAURI_SIGNING_PRIVATE_KEY': 'fixture-updater-secret',
               'APPLE_APP_SPECIFIC_PASSWORD': 'fixture-notary-secret',
               'APPLE_ID': 'fixture@example.test', 'APPLE_TEAM_ID': 'TESTTEAM'}
        with patch.dict(release.os.environ, env, clear=True):
            runner = release.Runner()
        self.assertNotIn('TAURI_SIGNING_PRIVATE_KEY', runner.env)
        self.assertNotIn('APPLE_APP_SPECIFIC_PASSWORD', runner.env)
        result = subprocess.CompletedProcess(['fixture'], 1, 'fixture-updater-secret', 'fixture-notary-secret')
        with patch.object(release.subprocess, 'run', return_value=result):
            with self.assertRaises(release.ReleaseError) as raised:
                runner.run(['fixture'])
        self.assertNotIn('fixture-updater-secret', str(raised.exception))
        self.assertNotIn('fixture-notary-secret', str(raised.exception))

    def test_publish_requires_both_notarizations_and_creates_latest_at_verified_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp); version = '0.1.5'
            tar = out / 'Dispatch-0.1.5-darwin-aarch64.app.tar.gz'; tar.write_bytes(b'final')
            Path(str(tar) + '.sig').write_text('public signature')
            zip_file = out / 'Dispatch-0.1.5-macos-arm64-signed.zip'; zip_file.write_bytes(b'zip')
            release.write_feed(out, 'v0.1.5', version, 'darwin-aarch64', 'notes', tar, zip_file)
            files = [tar, Path(str(tar) + '.sig'), zip_file, out / 'latest.json', out / 'SHA256SUMS.txt']
            release.save(out / 'verified.json', {'source': 'commit-one', 'files': {p.name: release.digest(p) for p in files}})
            (out / 'release-notes.md').write_text('release body')
            plan = {'source': 'commit-one', 'tag': 'v0.1.5', 'version': version,
                    'body_sha256': release.digest(out / 'release-notes.md')}
            for label in ['app', 'cli']:
                archive = out / (label + '.zip'); archive.write_bytes(label.encode())
                release.save(out / (label + '.notary.json'), {'status': 'Accepted', 'sha256': release.digest(archive)})
            calls = []
            class FakeRunner:
                def run(self, args, **kwargs):
                    calls.append(args)
                    return subprocess.CompletedProcess(args, 1 if args[1] == 'api' else 0, '', 'HTTP 404')
            runner = FakeRunner()
            release.save(out / 'cli.notary.json', {'status': 'Invalid', 'sha256': release.digest(out / 'cli.zip')})
            with self.assertRaises(release.ReleaseError): release.publish(out, plan, runner)
            self.assertFalse(calls)
            release.save(out / 'cli.notary.json', {'status': 'Accepted', 'sha256': release.digest(out / 'cli.zip')})
            release.publish(out, plan, runner)
            args = calls[-1]
            self.assertIn('--latest', args)
            self.assertNotIn('--prerelease', args)
            self.assertEqual(args[args.index('--target') + 1], 'commit-one')
            self.assertEqual(set(args[4:9]), set(files))
            calls.clear()
            (out / 'release-notes.md').write_text('changed notes')
            with self.assertRaises(release.ReleaseError): release.publish(out, plan, runner)
            self.assertFalse(calls)

    def test_version_mismatch_cannot_build_or_publish(self):
        release.validate_version('v0.1.5', '0.1.5', '0.1.5')
        for tag, cargo, app in [('v0.1.6', '0.1.5', '0.1.5'),
                                 ('v0.1.5', '0.1.5', '0.1.4'),
                                 ('v0.1.5-rc.1', '0.1.5-rc.1', '0.1.5-rc.1')]:
            with self.assertRaises(release.ReleaseError):
                release.validate_version(tag, cargo, app)

    def test_feed_escapes_notes_and_checksums_bind_all_four_files(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            tar = out / 'Dispatch-0.1.5-darwin-aarch64.app.tar.gz'
            tar.write_bytes(b'final notarized app archive')
            sig = Path(str(tar) + '.sig'); sig.write_text('public signature\n')
            zip_file = out / 'Dispatch-0.1.5-macos-arm64-signed.zip'; zip_file.write_bytes(b'zip')
            notes = 'Quote "this"\nand \\that'
            release.write_feed(out, 'v0.1.5', '0.1.5', 'darwin-aarch64', notes, tar, zip_file)
            feed = json.loads((out / 'latest.json').read_text())
            self.assertEqual(feed['notes'], notes)
            self.assertEqual(feed['platforms']['darwin-aarch64']['signature'], 'public signature')
            self.assertEqual(len((out / 'SHA256SUMS.txt').read_text().splitlines()), 4)
            release.verify_checksums(out)
            tar.write_bytes(b'tampered after signing')
            with self.assertRaises(release.ReleaseError):
                release.verify_checksums(out)

    def test_notary_rejection_and_timeout_never_resubmit(self):
        for state in ['Invalid', 'In Progress', 'Accepted']:
            with self.subTest(state=state), tempfile.TemporaryDirectory() as tmp:
                archive = Path(tmp) / 'app.zip'; archive.write_bytes(b'signed app')
                calls = []
                def api(args):
                    calls.append(args[0])
                    if args[0] == 'submit': return {'id': 'submission-one'}
                    if args[0] == 'wait': return {'status': state}
                    return {'issues': [{'message': 'controlled validation failure'}]}
                if state == 'Accepted':
                    release.notarize(archive, api)
                    release.notarize(archive, api)
                    self.assertEqual(calls, ['submit', 'wait'])
                else:
                    with self.assertRaises(release.ReleaseError): release.notarize(archive, api)
                    with self.assertRaises(release.ReleaseError): release.notarize(archive, api)
                    self.assertEqual(calls.count('submit'), 1)

    def test_uncertain_submission_and_changed_archive_stop_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            archive = Path(tmp) / 'cli.zip'; archive.write_bytes(b'signed cli')
            def interrupted(_): raise release.ReleaseError('connection lost after submit')
            with self.assertRaises(release.ReleaseError): release.notarize(archive, interrupted)
            def unexpected(_): self.fail('must not make another Apple request')
            with self.assertRaises(release.ReleaseError): release.notarize(archive, unexpected)
            archive.write_bytes(b'different cli')
            with self.assertRaises(release.ReleaseError): release.notarize(archive, unexpected)


if __name__ == '__main__':
    unittest.main()
