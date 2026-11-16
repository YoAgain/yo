"""Read-only update collection using sample files and command outputs."""
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import users


class UpdatesTests(unittest.TestCase):
    def test_apt_samples(self):
        self.assertEqual(users.apt_counts('warning\n12;3\n'), {"packages": 12, "security": 3})
        with self.assertRaises(ValueError):
            users.apt_counts('Could not open cache')

    def test_kernel_versions(self):
        self.assertGreater(users.kernel_key('6.8.0-100-generic'), users.kernel_key('6.8.0-99-generic'))

    def test_log_result(self):
        sample = ('2026-11-13 06:20:00 INFO Starting unattended upgrades script\n'
                  '2026-11-13 06:21:00 INFO All upgrades installed\nnoise\n')
        self.assertIn('06:21:00 INFO All upgrades installed', users.unattended_result(sample))
        self.assertNotIn('noise', users.unattended_result(sample))
        self.assertIn('ERROR', users.unattended_result('2026-11-13 06:21:00 ERROR dpkg failed'))

    def collect(self, directory, previous=None, now=2000000, release='New release \'26.04.1 LTS\' available.'):
        root = Path(directory)
        files = {'REBOOT_FILE': 'reboot', 'REBOOT_PACKAGES': 'packages', 'OS_RELEASE': 'os',
                 'UNATTENDED_LOG': 'log', 'RELEASE_CONFIG': 'config', 'RELEASE_CACHE': 'release'}
        env = {'ADMIN_UPDATES_' + k: str(root / v) for k, v in files.items()}
        env['ADMIN_UPDATES_KERNEL_GLOB'] = str(root / 'vmlinuz-*')
        if release is not None:
            (root / 'release').write_text(release)
        outputs = {'APT_COMMAND': (0, '4;2'), 'UNAME_COMMAND': (0, '6.8.0-99-generic\n'),
                   'UPGRADE_TIMER_COMMAND': (0, 'Mon 2026-11-16 06:00:00 UTC'),
                   'REBOOT_TIMER_COMMAND': (0, 'Sun 2026-11-22 08:00:00 UTC')}
        with patch.dict(os.environ, env), patch.object(users, 'update_command', side_effect=lambda key, default: outputs[key]) as command:
            result = users.updates(previous, now)
        return result, [call.args[0] for call in command.call_args_list]

    def fixture(self, directory):
        root = Path(directory)
        for name, text in {'os': 'PRETTY_NAME="Ubuntu 24.04.3 LTS"\n', 'config': '[DEFAULT]\nPrompt=lts\n',
                           'log': '2026-11-16 06:00:00 INFO No packages found that can be upgraded unattended\n',
                           'packages': 'linux-image-generic\n', 'vmlinuz-6.8.0-99-generic': '',
                           'vmlinuz-6.8.0-100-generic': ''}.items():
            (root / name).write_text(text)

    def test_collection_cache_and_aging(self):
        with tempfile.TemporaryDirectory() as directory:
            self.fixture(directory)
            first, calls = self.collect(directory)
            self.assertEqual(first['apt']['security'], 2)
            self.assertEqual(first['installed_kernel'], '6.8.0-100-generic')
            self.assertEqual(first['release']['available'], '26.04.1 LTS')
            self.assertEqual(first['status'], 'warn')
            second, calls = self.collect(directory, first, now=2000010)
            self.assertNotIn('APT_COMMAND', calls)
            third, calls = self.collect(directory, second, now=2000900)
            self.assertIn('APT_COMMAND', calls)
            old, calls = self.collect(directory, third, now=2000000 + 3 * 86400)
            self.assertEqual(old['status'], 'bad')

    def test_release_failure_and_lts_guard(self):
        with tempfile.TemporaryDirectory() as directory:
            self.fixture(directory)
            data, _ = self.collect(directory, release='')
            self.assertNotIn('available', data['release'])
            self.assertEqual(data['release']['result'], 'No new release found')
            Path(directory, 'config').write_text('Prompt=normal\n')
            data, calls = self.collect(directory)
            self.assertIn('Prompt=lts', data['release']['error'])

    def test_failed_apt_is_cached_without_losing_age(self):
        with tempfile.TemporaryDirectory() as directory:
            self.fixture(directory)
            first, _ = self.collect(directory)
            env = {"ADMIN_UPDATES_RELEASE_CONFIG": str(Path(directory, 'config'))}
            with patch.dict(os.environ, env), patch.object(users, 'update_command', side_effect=OSError('unavailable')):
                failed = users.updates(first, 2000900)
            self.assertEqual(failed['apt']['security_since'], first['apt']['security_since'])
            self.assertIn('unavailable', failed['apt']['error'])
            cached, calls = self.collect(directory, failed, now=2000910)
            self.assertNotIn('APT_COMMAND', calls)
            self.assertIn('error', cached['apt'])

    def test_thresholds(self):
        data = {'apt': {'packages': 0, 'security': 0}, 'release': {}}
        self.assertEqual(users.update_status(data, 1000000), 'ok')
        data.update(reboot=True, reboot_since=1)
        self.assertEqual(users.update_status(data, 7 * 86400 + 1), 'warn')
        self.assertEqual(users.update_status(data, 7 * 86400 + 2), 'bad')
        data = {'apt': {'packages': 1, 'security': 1, 'security_since': 1}}
        self.assertEqual(users.update_status(data, 2 * 86400 + 1), 'warn')
        self.assertEqual(users.update_status(data, 2 * 86400 + 2), 'bad')

    def test_reboot_marker(self):
        with tempfile.TemporaryDirectory() as directory:
            self.fixture(directory)
            marker = Path(directory, 'reboot')
            marker.touch()
            os.utime(marker, (1, 1))
            data, _ = self.collect(directory)
            self.assertEqual(data['reboot_packages'], ['linux-image-generic'])
            self.assertEqual(data['reboot_since'], 1)
            self.assertEqual(data['status'], 'bad')


if __name__ == '__main__':
    unittest.main()
