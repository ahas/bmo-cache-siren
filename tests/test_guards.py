"""Repository guards: the daemon runs on the standard library alone, and no Korean is written into any
text file of the checkout, from code and tests to the documents and root files (only the web unit
holds language packs; this one answers in English)."""
import ast
import os
import re
import sys
import tomllib
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "src")
# Folders holding version control, caches and build output, not files of the repository.
UNGUARDED_DIRS = {".git", "__pycache__", "dist", ".build", "target"}
# Hangul Jamo, Compatibility Jamo, Jamo Extended-A, Syllables and Jamo Extended-B, built from code
# points so this file holds none of the characters it looks for.
HANGUL_RANGES = ((0x1100, 0x11FF), (0x3130, 0x318F), (0xA960, 0xA97F), (0xAC00, 0xD7AF), (0xD7B0, 0xD7FF))
HANGUL = re.compile("[" + "".join(f"{chr(lo)}-{chr(hi)}" for lo, hi in HANGUL_RANGES) + "]")


def imported_modules(path):
    with open(path, encoding="utf-8") as f:
        tree = ast.parse(f.read(), path)
    for node in ast.walk(tree):
        if isinstance(node, ast.Import):
            for alias in node.names:
                yield alias.name.split(".")[0]
        elif isinstance(node, ast.ImportFrom) and node.level == 0 and node.module:
            yield node.module.split(".")[0]


class DependencyGuardTest(unittest.TestCase):
    def test_src_imports_only_the_standard_library(self):
        own = {name[:-3] for name in os.listdir(SRC) if name.endswith(".py")}
        for name in sorted(os.listdir(SRC)):
            if not name.endswith(".py"):
                continue
            for module in imported_modules(os.path.join(SRC, name)):
                with self.subTest(file=name, module=module):
                    self.assertTrue(module in sys.stdlib_module_names or module in own)

    def test_the_manifest_declares_no_dependencies(self):
        with open(os.path.join(ROOT, "pyproject.toml"), "rb") as f:
            project = tomllib.load(f)["project"]

        self.assertEqual(project.get("dependencies"), [])
        self.assertEqual(project.get("optional-dependencies", {}), {})


class NoHangulGuardTest(unittest.TestCase):
    def test_no_hangul_in_any_file_of_the_checkout(self):
        found = []
        for folder, dirs, files in os.walk(ROOT):
            dirs[:] = [d for d in dirs if d not in UNGUARDED_DIRS]
            for name in files:
                path = os.path.join(folder, name)
                try:
                    with open(path, encoding="utf-8") as f:
                        text = f.read()
                except (UnicodeDecodeError, OSError):
                    continue
                for n, line in enumerate(text.splitlines(), 1):
                    if HANGUL.search(line):
                        found.append(f"{os.path.relpath(path, ROOT)}:{n}")

        self.assertEqual(found, [])


if __name__ == "__main__":
    unittest.main()
