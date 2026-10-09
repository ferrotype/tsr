"""The README's release order publishes every retained dependency first."""
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import package_assets


class ReleaseOrderTests(unittest.TestCase):
    retained = {'tsr_lsp': {'tsr_api', 'tsr_core'}, 'tsr_api': {'tsr_core'}}
    public = {'tsr_core', 'tsr_api', 'tsr_lsp'}

    def test_dependencies_first_passes(self):
        package_assets.check_release_order(['tsr_core', 'tsr_api', 'tsr_lsp'], self.retained, self.public)

    def test_a_package_before_its_dependency_fails(self):
        with self.assertRaisesRegex(ValueError, 'lists tsr_lsp before its dependencies: tsr_api'):
            package_assets.check_release_order(['tsr_core', 'tsr_lsp', 'tsr_api'], self.retained, self.public)

    def test_missing_unknown_and_repeated_names_fail(self):
        for order in (['tsr_core', 'tsr_api'], ['tsr_core', 'tsr_api', 'tsr_lsp', 'tsr_ls'],
                      ['tsr_core', 'tsr_core', 'tsr_api', 'tsr_lsp']):
            with self.subTest(order=order), self.assertRaisesRegex(ValueError, 'every public package once'):
                package_assets.check_release_order(order, self.retained, self.public)

    def test_repository_order_publishes_dependencies_first(self):
        package_assets.publication_policy()


if __name__ == '__main__':
    unittest.main()
