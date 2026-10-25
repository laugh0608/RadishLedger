"""合成源码包验证嵌套许可不遗漏；不访问 Cargo 缓存或网络。"""
import importlib.util
from pathlib import Path
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("native_notices", ROOT / "scripts/collect-native-notices.py")
NOTICES = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(NOTICES)


class NativeNoticesTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="radishledger-notice-test-")
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name)
        (self.base / "sqlcipher").mkdir()
        self.package = {"name": "sqlcipher-src", "version": "synthetic",
                        "manifest_path": str(self.base / "Cargo.toml")}
        for name, text in {"LICENSE": "synthetic wrapper MIT",
                           "sqlcipher/LICENSE-sqlcipher": "synthetic SQLCipher BSD",
                           "sqlcipher/LICENSE-libtomcrypt": "synthetic libtomcrypt dual license",
                           "sqlcipher/sqlite3.h": "/*\n** The author disclaims copyright to this source code.\n** synthetic blessing\n*/\n#define SYNTHETIC 1\n"}.items():
            (self.base / name).write_text(text + "\n", encoding="utf-8")

    def collect(self):
        return NOTICES.package_notices(self.package, self.base, self.base / "unused")

    def test_nested_notices_and_sqlite_header_are_preserved(self):
        self.assertEqual(self.collect(), [
            "synthetic wrapper MIT\n", "synthetic SQLCipher BSD\n",
            "synthetic libtomcrypt dual license\n",
            "/*\n** The author disclaims copyright to this source code.\n** synthetic blessing\n*/\n",
        ])

    def test_missing_wrapper_or_nested_license_rejects_incomplete_notice(self):
        for name in ["LICENSE", "sqlcipher/LICENSE-sqlcipher", "sqlcipher/LICENSE-libtomcrypt"]:
            with self.subTest(name=name):
                path = self.base / name
                content = path.read_text()
                path.unlink()
                with self.assertRaises(FileNotFoundError):
                    self.collect()
                path.write_text(content)

    def test_empty_nested_license_rejects_incomplete_notice(self):
        (self.base / "sqlcipher/LICENSE-libtomcrypt").write_text(" \n")
        with self.assertRaisesRegex(SystemExit, "许可文件为空"):
            self.collect()

    def test_missing_or_changed_sqlite_notice_rejects_generation(self):
        for header in ["#define SYNTHETIC 1\n", "/* no copyright declaration */\n",
                       "/* The author disclaims copyright to this source code.\n"]:
            with self.subTest(header=header):
                (self.base / "sqlcipher/sqlite3.h").write_text(header)
                with self.assertRaisesRegex(SystemExit, "SQLite 版权声明缺失"):
                    self.collect()

    def test_openssl_build_tool_license_is_required_and_preserved(self):
        self.package["name"] = "openssl-src"
        nested = self.base / "openssl/external/perl/Text-Template-1.56/LICENSE"
        nested.parent.mkdir(parents=True)
        (self.base / "openssl/LICENSE.txt").write_text("synthetic OpenSSL Apache\n")
        with self.assertRaises(FileNotFoundError):
            self.collect()
        nested.write_text(" \n")
        with self.assertRaisesRegex(SystemExit, "许可文件为空"):
            self.collect()
        nested.write_text("synthetic Text::Template dual license\n")
        self.assertIn("synthetic Text::Template dual license\n", self.collect())


if __name__ == "__main__":
    unittest.main()
