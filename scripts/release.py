#!/usr/bin/env python3
"""macOS release pipeline. Invoked by release.sh; secrets only arrive in the environment."""
import argparse
import base64
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile

REPO = 'IvyYang1999/dispatch'
ROOT = Path(__file__).resolve().parent.parent


class ReleaseError(Exception):
    pass


def require(condition, message):
    if not condition:
        raise ReleaseError(message)


def digest(file):
    return hashlib.sha256(file.read_bytes()).hexdigest()


def save(file, data):
    # Receipts survive an interrupted wait; never persist credentials or argv.
    tmp = file.with_suffix(file.suffix + '.tmp')
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2) + '\n', encoding='utf-8')
    tmp.replace(file)


def validate_version(tag, version, app_version):
    require(re.fullmatch(r'v\d+\.\d+\.\d+', tag) and tag == 'v' + version == 'v' + app_version,
            'Tag, workspace and app versions must match exactly (stable vX.Y.Z); no source files are rewritten.')


def write_feed(out, tag, version, platform, notes, tar, zip_file):
    sig = Path(str(tar) + '.sig')
    feed = out / 'latest.json'
    save(feed, {'version': version, 'notes': notes,
                'pub_date': datetime.datetime.now(datetime.timezone.utc).isoformat().replace('+00:00', 'Z'),
                'platforms': {platform: {'signature': sig.read_text().strip(),
                    'url': 'https://github.com/' + REPO + '/releases/download/' + tag + '/' + tar.name}}})
    files = [tar, sig, zip_file, feed]
    (out / 'SHA256SUMS.txt').write_text(''.join(digest(f) + '  ' + f.name + '\n' for f in files))


def verify_checksums(out):
    rows = (out / 'SHA256SUMS.txt').read_text().splitlines()
    require(len(rows) == 4, 'Expected exactly four release checksums')
    names = set()
    for row in rows:
        checksum, name = row.split('  ', 1)
        require(Path(name).name == name and name not in names, 'Invalid checksum filename')
        require(digest(out / name) == checksum, 'Release file changed: ' + name)
        names.add(name)


def notarize(archive, api):
    receipt = archive.with_suffix('.notary.json')
    sha = digest(archive)
    if receipt.exists():
        data = json.loads(receipt.read_text())
        require(data['sha256'] == sha, 'Notary archive changed; refusing to reuse or resubmit it')
        require(data.get('id'), 'Submission outcome unknown. Inspect notarytool history/log before recovery; do not resubmit.')
    else:
        data = {'sha256': sha, 'id': None, 'status': 'Submitting'}
        save(receipt, data)  # A timeout after submit is NOT authorization for a second upload.
        result = api(['submit', str(archive), '--no-wait'])
        require(result.get('id'), 'No submission ID returned; submission outcome is unknown')
        data['id'] = result['id']
        data['status'] = 'In Progress'
        save(receipt, data)
    if data['status'] == 'Accepted':
        return
    if data['status'] == 'Invalid':
        raise ReleaseError('Notarization rejected; see ' + str(archive.with_suffix('.log.json')))
    result = api(['wait', data['id'], '--timeout', '15m'])
    data['status'] = result.get('status', 'Unknown')
    save(receipt, data)
    print(archive.name + ': ' + data['status'], flush=True)
    if data['status'] == 'Invalid':
        log = api(['log', data['id']])
        save(archive.with_suffix('.log.json'), log)
        raise ReleaseError('Notarization rejected: ' + json.dumps(log.get('issues', []), ensure_ascii=False))
    require(data['status'] == 'Accepted', 'Notarization is not Accepted; resume this directory to wait on the same submission.')


class Runner:
    def __init__(self):
        self.env = dict(os.environ)
        self.key = self.env.pop('TAURI_SIGNING_PRIVATE_KEY', '')
        self.key_password = self.env.pop('TAURI_SIGNING_PRIVATE_KEY_PASSWORD', '')
        password = self.env.pop('APPLE_APP_SPECIFIC_PASSWORD', '')
        apple_id = self.env.pop('APPLE_ID', '')
        team = self.env.pop('APPLE_TEAM_ID', '')
        self.identity = self.env.pop('APPLE_SIGNING_IDENTITY', '')
        self.env.pop('APPLE_PASSWORD', None)
        self.secrets = [s for s in [self.key, self.key_password, password] if s]
        profile = self.env.get('NOTARYTOOL_PROFILE')
        if profile:
            self.auth = ['--keychain-profile', profile]
        else:
            require(apple_id and team and password, 'Set NOTARYTOOL_PROFILE or inject apple-notary with KeyKeeper.')
            self.auth = ['--apple-id', apple_id, '--team-id', team, '--password', password]
        require(self.key, 'Inject TAURI_SIGNING_PRIVATE_KEY with KeyKeeper (no default private-key file).')

    def run(self, args, *, timeout=600, sensitive=False, check=True, env=None):
        try:
            result = subprocess.run([str(a) for a in args], cwd=ROOT, env=env or self.env,
                                    capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise ReleaseError(Path(str(args[0])).name + ' timed out; no automatic retry') from None
        for secret in self.secrets:
            result.stdout = result.stdout.replace(secret, '[REDACTED]')
            result.stderr = result.stderr.replace(secret, '[REDACTED]')
        if check and result.returncode:
            detail = '' if sensitive else '\n' + (result.stdout + result.stderr)[-4000:]
            raise ReleaseError(Path(str(args[0])).name + ' failed' + detail)
        return result

    def notary(self, args):
        r = self.run(['xcrun', 'notarytool', *args, *self.auth, '--output-format', 'json'],
                     timeout=930 if args[0] == 'wait' else 180, sensitive=True, check=False)
        try:
            data = json.loads(r.stdout)
        except ValueError:
            raise ReleaseError('notarytool failed: ' + r.stderr[-2000:]) from None
        require(not r.returncode or data.get('status') in ['Invalid', 'In Progress'],
                'notarytool failed; preserve this run and inspect its submission before retrying')
        return data


def build_archives(out, runner, identity):
    print('Building CLI and app (updater signing happens only after notarization)', flush=True)
    runner.run(['node', 'scripts/sidecar.mjs'], timeout=1200)
    runner.run(['npm', '--prefix', 'apps/desktop', 'run', 'tauri', 'build', '--', '--bundles', 'app', '--config',
                json.dumps({'bundle': {'createUpdaterArtifacts': False, 'macOS': {'signingIdentity': '-'}}})], timeout=1800)
    with tempfile.TemporaryDirectory(prefix='prepare-', dir=out) as tmp:
        app = Path(tmp) / 'Dispatch.app'
        runner.run(['ditto', ROOT / 'target/release/bundle/macos/Dispatch.app', app])
        embedded = app / 'Contents/MacOS/dispatch'
        # Tauri replaces the linker's ad-hoc signature. Use the just-built CLI as
        # the canonical input before our own inside-out Developer ID signing.
        runner.run(['ditto', ROOT / 'target/release/dispatch', embedded])
        require(embedded.read_bytes() == (ROOT / 'target/release/dispatch').read_bytes(), 'Bundled CLI does not match this build')
        for item in [embedded, app]:
            runner.run(['codesign', '--force', '--options', 'runtime', '--timestamp', '--sign', identity, item])
        runner.run(['codesign', '--verify', '--deep', '--strict', '--verbose=2', app])
        runner.run(['codesign', '--verify', '--strict', '--verbose=2', embedded])
        runner.run(['ditto', '-c', '-k', '--keepParent', app, out / 'app.zip'])
        runner.run(['ditto', '-c', '-k', '--keepParent', embedded, out / 'cli.zip'])
    save(out / 'prepared.json', {name: digest(out / name) for name in ['app.zip', 'cli.zip']})


def manifest(directory):
    return {str(p.relative_to(directory)): ({'link': os.readlink(p)} if p.is_symlink() else
            {'sha256': digest(p), 'mode': p.stat().st_mode & 0o777})
            for p in sorted(directory.rglob('*')) if p.is_symlink() or p.is_file()}


def unpack_payload(out, package, runner):
    runner.run(['ditto', '-x', '-k', out / 'app.zip', package])
    # ditto --keepParent preserves the CLI's MacOS/ parent. That internal
    # notarization layout must not leak into the first-install package.
    with tempfile.TemporaryDirectory(prefix='cli-', dir=package.parent) as tmp:
        runner.run(['ditto', '-x', '-k', out / 'cli.zip', tmp])
        candidates = [p for p in Path(tmp).rglob('dispatch') if p.is_file()]
        require(len(candidates) == 1, 'Expected one CLI in the notarized archive')
        runner.run(['ditto', candidates[0], package / 'dispatch'])


def finish(out, plan, config, runner):
    prepared = json.loads((out / 'prepared.json').read_text())
    for name in ['app.zip', 'cli.zip']:
        require(digest(out / name) == prepared[name], 'Prepared notarization archive changed')
        notarize(out / name, runner.notary)
    print('Both notarizations Accepted; stapling and checking Gatekeeper', flush=True)
    version, arch, target = plan['version'], plan['arch'], plan['platform']
    with tempfile.TemporaryDirectory(prefix='package-', dir=out) as tmp:
        package = Path(tmp) / ('Dispatch-' + version + '-macos-' + arch)
        package.mkdir()
        unpack_payload(out, package, runner)
        app = package / 'Dispatch.app'
        cli = package / 'dispatch'
        info = plistlib.loads((app / 'Contents/Info.plist').read_bytes())
        require(info['CFBundleShortVersionString'] == version, 'Built app version mismatch')
        require(cli.read_bytes() == (app / 'Contents/MacOS/dispatch').read_bytes(), 'Standalone and bundled CLIs differ')
        runner.run(['xcrun', 'stapler', 'staple', app])
        runner.run(['xcrun', 'stapler', 'validate', app])
        runner.run(['codesign', '--verify', '--deep', '--strict', '--verbose=2', app])
        runner.run(['spctl', '-a', '-vv', '-t', 'exec', app])
        # spctl's exec policy expects an app; this is Apple's check for other code.
        runner.run(['codesign', '--verify', '--strict', '--verbose=2', '-R=notarized', '--check-notarization', cli])
        (package / '安装.txt').write_text(
            'Dispatch ' + version + '\n\n'
            'App 和命令行均已 Developer ID 签名、公证。\n'
            '1. 把 Dispatch.app 拖到「应用程序」。\n'
            '2. 首次使用：打开 App → 设置 → Agent，点「一键安装」；或者运行 ./dispatch install。\n'
            '   以后 App 更新并重启时会自动同步已经安装的 CLI。\n'
            '3. 插件：在同一页 Claude Code 和 Codex 各有「安装插件」按钮。自己动手是：\n'
            '   Claude Code 里依次输入 /plugin marketplace add IvyYang1999/dispatch\n'
            '   和 /plugin install dispatch@dispatch\n'
            '   Codex：codex plugin marketplace add IvyYang1999/dispatch && codex plugin add dispatch@dispatch\n', encoding='utf-8')
        zip_file = out / (package.name + '-signed.zip')
        tar = out / ('Dispatch-' + version + '-' + target + '.app.tar.gz')
        zip_file.unlink(missing_ok=True)
        runner.run(['ditto', '-c', '-k', '--keepParent', package, zip_file])
        runner.run(['tar', '-czf', tar, '-C', package, 'Dispatch.app'], env=dict(runner.env, COPYFILE_DISABLE='1'))
        print('Signing the final notarized updater archive', flush=True)
        runner.run([ROOT / 'apps/desktop/node_modules/.bin/tauri', 'signer', 'sign', tar, '--app-version', version],
                   sensitive=True, env=dict(runner.env, TAURI_SIGNING_PRIVATE_KEY=runner.key,
                                           TAURI_SIGNING_PRIVATE_KEY_PASSWORD=runner.key_password))
        pub = Path(tmp) / 'updater.pub'; sig = Path(tmp) / 'update.minisig'
        pub.write_bytes(base64.b64decode(config['plugins']['updater']['pubkey'], validate=True))
        sig.write_bytes(base64.b64decode(Path(str(tar) + '.sig').read_text().strip(), validate=True))
        runner.run(['minisign', '-Vm', tar, '-p', pub, '-x', sig])
        require(('version:' + version) in sig.read_text().splitlines()[2].split(), 'Updater signature version mismatch')
        unpacked = Path(tmp) / 'unpacked'; unpacked.mkdir()
        runner.run(['ditto', '-x', '-k', zip_file, unpacked])
        require(manifest(unpacked / package.name) == manifest(package), 'First-install archive differs from signed payload')
        extracted = Path(tmp) / 'updater'; extracted.mkdir()
        runner.run(['tar', '-xzf', tar, '-C', extracted])
        require(manifest(extracted / 'Dispatch.app') == manifest(app), 'Updater archive differs from notarized app')
        write_feed(out, plan['tag'], version, target, plan['notes'], tar, zip_file)
        verify_checksums(out)
        save(out / 'verified.json', {'source': plan['source'], 'app': manifest(app),
                                    'files': {p.name: digest(p) for p in [tar, Path(str(tar) + '.sig'), zip_file, out / 'latest.json', out / 'SHA256SUMS.txt']}})


def require_new_tag(tag, runner):
    ref = runner.run(['gh', 'api', 'repos/' + REPO + '/git/ref/tags/' + tag], check=False)
    require(ref.returncode != 0 and 'HTTP 404' in ref.stderr, 'Tag already exists or GitHub could not verify it; refusing publication')


def publish(out, plan, runner):
    verify_checksums(out)
    receipt = json.loads((out / 'verified.json').read_text())
    require(receipt['source'] == plan['source'], 'Verified build source changed')
    require(hashlib.sha256((out / 'release-notes.md').read_bytes()).hexdigest() == plan['body_sha256'], 'Release notes changed')
    for name, sha in receipt['files'].items():
        require(digest(out / name) == sha, 'Verified artifact changed before publish')
    for name in ['app', 'cli']:
        accepted = json.loads((out / (name + '.notary.json')).read_text())
        require(accepted['status'] == 'Accepted' and accepted['sha256'] == digest(out / (name + '.zip')),
                'Both archives must be notarized before publishing')
    # --target is ignored by GitHub when a tag already exists. Do not publish a
    # candidate against a pre-existing tag, even if no Release exists for it yet.
    require_new_tag(plan['tag'], runner)
    runner.run(['gh', 'release', 'create', plan['tag'], *[out / n for n in receipt['files']],
                '--repo', REPO, '--target', plan['source'], '--title', 'Dispatch ' + plan['version'],
                '--latest', '--notes-file', out / 'release-notes.md'])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('tag')
    parser.add_argument('notes', nargs='?', default='')
    parser.add_argument('--notes-file', type=Path, help='Release body; positional notes are the updater summary')
    parser.add_argument('--resume', action='store_true', help='Resume this commit/tag run; never resubmit an uncertain upload')
    args = parser.parse_args()
    require(platform.system() == 'Darwin', 'This pipeline runs on macOS; Linux/Windows use release.yml')
    require(os.environ.get('PUBLISH', '0') in ['0', '1'], 'PUBLISH must be 0 or 1')
    require(not os.environ.get('EXTRA_TAURI_CONFIG'), 'Release builds must use the committed Tauri configuration')
    version = re.search(r'^version\s*=\s*"([^"]+)"', (ROOT / 'Cargo.toml').read_text(), re.M).group(1)
    config = json.loads((ROOT / 'apps/desktop/src-tauri/tauri.conf.json').read_text())
    validate_version(args.tag, version, config['version'])
    require(config['plugins']['updater']['endpoints'] == ['https://github.com/' + REPO + '/releases/latest/download/latest.json'],
            'Release updater endpoint does not match this repository')
    for tool in ['cargo', 'node', 'npm', 'ditto', 'codesign', 'xcrun', 'spctl', 'security', 'tar', 'minisign', 'git']:
        require(shutil.which(tool), 'Required tool missing: ' + tool)
    if os.environ.get('PUBLISH') == '1':
        require(shutil.which('gh'), 'Publishing requires gh with an existing GitHub login')
    runner = Runner()
    if os.environ.get('PUBLISH') == '1':
        require_new_tag(args.tag, runner)
    require(not runner.run(['git', 'status', '--porcelain']).stdout.strip(), 'Commit the candidate first; release builds require a clean checkout')
    sha = runner.run(['git', 'rev-parse', 'HEAD']).stdout.strip()
    arch = platform.machine(); require(arch in ['arm64', 'x86_64'], 'Unsupported Mac architecture')
    target = 'darwin-aarch64' if arch == 'arm64' else 'darwin-x86_64'
    identities = re.findall(r'"(Developer ID Application: [^"]+)"', runner.run(['security', 'find-identity', '-v', '-p', 'codesigning']).stdout)
    identity = runner.identity or (identities[0] if len(identities) == 1 else '')
    require(identity in identities, 'Choose an available Developer ID Application identity with APPLE_SIGNING_IDENTITY')
    notes = args.notes or 'Dispatch ' + version
    body = args.notes_file.read_text(encoding='utf-8') if args.notes_file else notes
    plan = {'tag': args.tag, 'version': version, 'arch': arch, 'platform': target, 'source': sha, 'notes': notes, 'body_sha256': hashlib.sha256(body.encode()).hexdigest()}
    out = ROOT / 'dist' / ('release-' + args.tag + '-' + sha[:12])
    out.parent.mkdir(exist_ok=True)
    if args.resume:
        require(out.is_dir() and json.loads((out / 'plan.json').read_text()) == plan, 'Resume must use the same source, tag and notes')
    else:
        require(not out.exists(), 'Run directory already exists; use --resume: ' + str(out))
        out.mkdir(mode=0o700)
        save(out / 'plan.json', plan)
        (out / 'release-notes.md').write_text(body, encoding='utf-8')
    print('Release run: ' + str(out), flush=True)
    import fcntl
    with (out / '.lock').open('w') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise ReleaseError('This release run is already active') from None
        if not (out / 'prepared.json').exists():
            require(not list(out.glob('*.notary.json')), 'Cannot rebuild after a notarization request')
            build_archives(out, runner, identity)
        finish(out, plan, config, runner)
        require(runner.run(['git', 'rev-parse', 'HEAD']).stdout.strip() == sha and
                not runner.run(['git', 'status', '--porcelain']).stdout.strip(), 'Source changed during the build; refusing publication')
        if os.environ.get('PUBLISH') == '1':
            publish(out, plan, runner)
            print('Published https://github.com/' + REPO + '/releases/tag/' + args.tag)
        else:
            print('Verified build only; nothing published. Five release assets in ' + str(out))


if __name__ == '__main__':
    try:
        main()
    except (ReleaseError, OSError, ValueError, KeyError) as error:
        print('Release stopped: ' + str(error), file=sys.stderr)
        sys.exit(1)
