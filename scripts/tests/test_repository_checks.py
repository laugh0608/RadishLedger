"""Synthetic rejection tests; never change the developer's repository."""

import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("repository_checks", ROOT / "scripts/check-repo.py")
CHECKS = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(CHECKS)


class RepositoryChecksTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="radishledger-check-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name in CHECKS.repository_files(ROOT):
            destination = self.root / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, destination)
        self.run_git("init", "-q", "-b", "dev")
        self.run_git("config", "user.name", "Synthetic Fixture")
        self.run_git("config", "user.email", "fixture@example.invalid")

    def run_git(self, *args):
        return CHECKS.git(self.root, *args).decode().strip()

    def write(self, name, value):
        target = self.root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(value, encoding="utf-8")

    def inspect(self):
        files = CHECKS.repository_files(self.root)
        errors, texts, documents = CHECKS.check_text(self.root, files)
        return files, errors, texts, documents

    def make_commit(self, name, value, message):
        self.write(name, value)
        self.run_git("add", "--", name)
        self.run_git("commit", "-q", "-m", message)
        return self.run_git("rev-parse", "HEAD")

    def test_uncommitted_baseline_has_real_checks(self):
        files, errors, texts, documents = self.inspect()
        self.assertFalse(errors)
        self.assertFalse(CHECKS.check_docs(self.root, files, texts))
        self.assertFalse(CHECKS.check_governance(self.root, files, documents))

    def test_native_patch_rejects_source_drift_and_extra_files(self):
        source = "third-party/openssl-sys/src/lib.rs"
        original = (self.root / source).read_text()
        self.write(source, original + "// synthetic unauthorized change\n")
        files, _, _, documents = self.inspect()
        self.assertTrue(any("unapproved upstream source change" in e
                            for e in CHECKS.check_openssl_patch(self.root, files, documents)))
        self.write(source, original)
        self.write("third-party/openssl-sys/extra.rs", "// synthetic\n")
        files, _, _, documents = self.inspect()
        self.assertTrue(any("source inventory differs" in e
                            for e in CHECKS.check_openssl_patch(self.root, files, documents)))

    def test_native_patch_rejects_unselected_or_widened_requirement(self):
        name = "third-party/openssl-sys/Cargo.toml"
        self.write(name, (self.root / name).read_text().replace('"=400.0.2"', '"400"'))
        files, _, _, documents = self.inspect()
        self.assertTrue(any("unexpected source requirement" in e
                            for e in CHECKS.check_openssl_patch(self.root, files, documents)))
        name = "Cargo.toml"
        self.write(name, (self.root / name).read_text().replace('path = "third-party/openssl-sys"',
                                                              'path = "third-party/unreviewed"'))
        files, _, _, documents = self.inspect()
        self.assertTrue(any("does not select" in e
                            for e in CHECKS.check_openssl_patch(self.root, files, documents)))

    def test_untracked_included_ignored_excluded_tracked_never_hidden(self):
        self.write("bad.md", "synthetic\r\n")
        self.write("local-data/private.txt", "ignored synthetic\r\n")
        files, errors, _, _ = self.inspect()
        self.assertIn("bad.md", files)
        self.assertNotIn("local-data/private.txt", files)
        self.assertTrue(any("bad.md" in error for error in errors))
        self.run_git("add", "-f", "--", "local-data/private.txt")
        files, errors, _, _ = self.inspect()
        self.assertIn("local-data/private.txt", files)
        self.assertTrue(any("local-data/private.txt" in error for error in errors))

    def test_bom_invalid_utf8_nul_and_newline_are_rejected(self):
        cases = (b"\xef\xbb\xbftext\n", b"\xff\n", b"text\0\n", b"text")
        for raw in cases:
            with self.subTest(raw=raw):
                (self.root / "invalid.txt").write_bytes(raw)
                self.assertTrue(CHECKS.check_text(self.root, ["invalid.txt"])[0])

    def test_json_duplicate_keys_fail(self):
        self.write("duplicate.json", '{"amount": 1, "amount": 2}\n')
        self.assertTrue(any("duplicate JSON key" in error for error in self.inspect()[1]))

    def test_nonstandard_json_numbers_fail(self):
        for constant in ("NaN", "Infinity", "-Infinity"):
            self.write("constant.json", '{"value": ' + constant + '}\n')
            self.assertTrue(CHECKS.check_text(self.root, ["constant.json"])[0])

    def test_forced_ignored_artifact_is_rejected(self):
        self.write("local-data/synthetic.db", "synthetic only\n")
        self.run_git("add", "-f", "--", "local-data/synthetic.db")
        files, _, _, documents = self.inspect()
        failures = CHECKS.check_governance(self.root, files, documents)
        self.assertTrue(any("ignored artifact is tracked" in error for error in failures))

    def test_markdown_hard_break_allowed_other_trailing_space_rejected(self):
        self.write("sample.md", "text  \n")
        self.assertFalse(CHECKS.check_text(self.root, ["sample.md"])[0])
        self.write("sample.md", "text \n")
        self.assertTrue(CHECKS.check_text(self.root, ["sample.md"])[0])

    def test_links_anchors_and_fenced_examples(self):
        self.write("links.md", "# Example\n\n[x](#example)\n\n```text\n[x](missing.md)\n```\n")
        files, _, texts, _ = self.inspect()
        self.assertFalse(CHECKS.check_docs(self.root, files, texts))
        self.write("links.md", "[x](missing.md)\n[x](README.md#absent)\n[x](../outside.md)\n")
        files, _, texts, _ = self.inspect()
        failures = CHECKS.check_docs(self.root, files, texts)
        self.assertTrue(any("missing or ignored" in error for error in failures))
        self.assertTrue(any("missing Markdown anchor" in error for error in failures))
        self.assertTrue(any("escapes repository" in error for error in failures))

    def test_fence_table_and_root_drift_fail(self):
        self.write("broken.md", "| A | B |\n| --- | --- |\n| C |\n\n```text\n")
        self.write("CLAUDE.md", "Different root entry\n")
        files, _, texts, _ = self.inspect()
        failures = CHECKS.check_docs(self.root, files, texts)
        for reason in ("unclosed code fence", "table row width", "must be identical"):
            self.assertTrue(any(reason in error for error in failures), reason)

    def test_remote_policy_drift_fails(self):
        files, _, _, documents = self.inspect()
        mutations = (
            (".github/repository-settings.json", lambda d: d.update(allow_squash_merge=True)),
            (".github/repository-settings.json", lambda d: d.update(delete_branch_on_merge=True)),
            (".github/rulesets/master-protection.json", lambda d: d["conditions"]["ref_name"].update(include=["refs/heads/dev"])),
            (".github/rulesets/master-protection.json", lambda d: d["bypass_actors"][0].update(bypass_mode="always")),
            (
                ".github/rulesets/master-protection.json",
                lambda d: d["rules"][-1]["parameters"].update(strict_required_status_checks_policy=False),
            ),
            (
                ".github/rulesets/master-protection.json",
                lambda d: d["rules"][-1]["parameters"]["required_status_checks"][0].update(context="Fake Success"),
            ),
            (
                ".github/rulesets/master-protection.json",
                lambda d: d["rules"][-1]["parameters"]["required_status_checks"][0].pop("integration_id"),
            ),
            (
                ".github/rulesets/master-protection.json",
                lambda d: d["rules"][-1]["parameters"]["required_status_checks"][0].update(integration_id=0),
            ),
            (
                ".github/rulesets/master-protection.json",
                lambda d: d["rules"][-1]["parameters"]["required_status_checks"][0].update(integration_id=True),
            ),
        )
        for name, mutate in mutations:
            with self.subTest(name=name):
                changed = copy.deepcopy(documents)
                mutate(changed[name])
                self.assertTrue(CHECKS.check_governance(self.root, files, changed))

    def test_workflow_drift_and_error_masking_fail(self):
        files, _, _, documents = self.inspect()
        for mutate in (
            lambda w: w["on"].update(push={}),
            lambda w: w["permissions"].update(contents="write"),
            lambda w: w["jobs"]["repo-hygiene"].update(**{"continue-on-error": True}),
            lambda w: w["jobs"]["repo-hygiene"]["steps"][0]["with"].update(**{"persist-credentials": True}),
            lambda w: w["jobs"]["candidate-quality"].update(needs=[]),
            lambda w: w["jobs"]["candidate-quality"].update(**{"if": "success()"}),
            lambda w: w["jobs"]["candidate-quality"]["steps"][0].update(run="exit 0"),
        ):
            changed = copy.deepcopy(documents)
            mutate(changed[".github/workflows/pr-check.yml"])
            self.assertTrue(CHECKS.check_governance(self.root, files, changed))

    def test_generated_workflow_edits_are_rejected(self):
        path = self.root / CHECKS.WORKFLOW_FILE
        self.write(CHECKS.WORKFLOW_FILE, path.read_text(encoding="utf-8") + "on: push\n")
        self.assertTrue(any("generated YAML drift" in error for error in self.inspect()[1]))

    def test_core_component_cannot_be_skipped_or_replaced_by_success(self):
        files, _, _, documents = self.inspect()
        mutations = (
            lambda w: w["jobs"]["core-quality"].update(**{"if": "false"}),
            lambda w: w["jobs"]["core-quality"]["steps"][2].update(run="exit 0"),
            lambda w: w["jobs"]["core-quality"]["steps"][3].update(run="exit 0"),
            lambda w: w["jobs"]["candidate-quality"].update(needs=["repo-hygiene"]),
        )
        for mutate in mutations:
            changed = copy.deepcopy(documents)
            mutate(changed[".github/workflows/pr-check.yml"])
            self.assertTrue(CHECKS.check_governance(self.root, files, changed))

    def test_version_mirrors_must_follow_the_root_source(self):
        path = self.root / "Cargo.toml"
        original = path.read_text()
        version = json.loads((self.root / "version.json").read_text())["productVersion"]
        changed = original.replace(f'version = "{version}"', 'version = "0.0.0"')
        self.assertNotEqual(original, changed)
        self.write("Cargo.toml", changed)
        files, _, _, documents = self.inspect()
        self.assertTrue(any("Cargo mirror drift" in error for error in CHECKS.check_governance(self.root, files, documents)))
        self.write("Cargo.toml", original)
        files, _, _, documents = self.inspect()
        documents["apps/m0_probe/pubspec.yaml"]["version"] = "0.0.0+1"
        self.assertTrue(any("Flutter mirror drift" in error for error in CHECKS.check_governance(self.root, files, documents)))

    def test_regenerated_workflow_cannot_bypass_policy(self):
        source = json.loads((self.root / CHECKS.WORKFLOW_SOURCE).read_text(encoding="utf-8"))
        source["on"]["push"] = {}
        self.write(CHECKS.WORKFLOW_SOURCE, json.dumps(source) + "\n")
        self.write(CHECKS.WORKFLOW_FILE, CHECKS.workflow_yaml(source))
        files, errors, _, documents = self.inspect()
        self.assertFalse(errors)
        self.assertTrue(any("only PR and manual triggers" in error for error in CHECKS.check_governance(self.root, files, documents)))

    def test_extra_workflow_requires_explicit_contract_review(self):
        self.write(".github/workflows/unreviewed.yml", '{"on": {"push": {}}}\n')
        files, _, _, documents = self.inspect()
        failures = CHECKS.check_governance(self.root, files, documents)
        self.assertTrue(any("workflow inventory changed" in error for error in failures))

    def test_actual_aggregate_rejects_failure_cancel_skip_missing(self):
        workflow = json.loads((ROOT / CHECKS.WORKFLOW_SOURCE).read_text())
        script = workflow["jobs"]["candidate-quality"]["steps"][0]["run"]
        cases = [({"repo-hygiene": {"result": "success"}, "core-quality": {"result": "success"}}, 0), ({}, 1)]
        cases += [({"repo-hygiene": {"result": value}}, 1) for value in ("failure", "cancelled", "skipped")]
        cases += [({"repo-hygiene": {}}, 1), ({"repo-hygiene": {"result": "success"}, "extra": {"result": "failure"}}, 1)]
        cases += [({"repo-hygiene": {"result": "success"}, "core-quality": {"result": value}}, 1) for value in ("failure", "cancelled", "skipped")]
        for results, expected in cases:
            with self.subTest(results=results):
                env = dict(os.environ, NEEDS_JSON=json.dumps(results))
                result = subprocess.run(["bash", "-e", "-c", script], env=env, capture_output=True, timeout=10)
                self.assertEqual(result.returncode, expected)

    def test_language_implementation_requires_real_component(self):
        self.write("crates/core/src/lib.rs", "pub fn future() {}\n")
        files, _, _, documents = self.inspect()
        failures = CHECKS.check_governance(self.root, files, documents)
        self.assertTrue(any("no quality component" in error for error in failures))

    def test_bad_commit_cannot_spoof_a_merge(self):
        base = self.make_commit("fixture.txt", "initial\n", "chore: initial fixture")
        head = self.make_commit("fixture.txt", "changed\n", "Merge fake message")
        self.assertTrue(CHECKS.commit_errors(self.root, base, head))

    def test_ai_attribution_rejected(self):
        base = self.make_commit("fixture.txt", "initial\n", "chore: initial fixture")
        head = self.make_commit("fixture.txt", "changed\n", "docs: update fixture\n\nCo-authored-by: Codex <codex@example.invalid>")
        failures = CHECKS.commit_errors(self.root, base, head)
        self.assertTrue(any("AI co-author" in error for error in failures))

    def test_pr_range_excludes_base_branch_and_actual_merge_commit(self):
        self.make_commit("fixture.txt", "initial\n", "chore: initial fixture")
        self.run_git("switch", "-q", "-c", "topic")
        head = self.make_commit("topic.txt", "topic\n", "docs: add topic fixture")
        self.run_git("switch", "-q", "dev")
        base = self.make_commit("base.txt", "base\n", "existing base history outside PR")
        self.run_git("switch", "-q", "topic")
        self.run_git("merge", "-q", "--no-ff", "dev", "-m", "Merge base into candidate")
        self.assertFalse(CHECKS.commit_errors(self.root, base, head))
        self.assertFalse(CHECKS.commit_errors(self.root, base, "HEAD"))

    def test_stage_promotion_fast_forward_backflow(self):
        self.make_commit("fixture.txt", "initial\n", "chore: initial fixture")
        self.run_git("branch", "master")
        feature = self.make_commit("feature.txt", "feature\n", "feat: add synthetic feature")
        self.run_git("switch", "-q", "master")
        self.run_git("merge", "-q", "--no-ff", "dev", "-m", "Merge dev promotion")
        promoted = self.run_git("rev-parse", "HEAD")
        self.run_git("switch", "-q", "dev")
        self.run_git("merge", "-q", "--ff-only", "master")
        self.assertEqual(self.run_git("rev-parse", "HEAD"), promoted)
        self.run_git("merge-base", "--is-ancestor", feature, "dev")

    def test_hotfix_divergence_uses_merge_without_losing_dev_history(self):
        self.make_commit("fixture.txt", "initial\n", "chore: initial fixture")
        self.run_git("branch", "master")
        feature = self.make_commit("feature.txt", "feature\n", "feat: add synthetic feature")
        self.run_git("switch", "-q", "master")
        hotfix = self.make_commit("hotfix.txt", "hotfix\n", "fix: synthetic stable correction")
        self.run_git("switch", "-q", "dev")
        with self.assertRaises(ValueError):
            self.run_git("merge", "--ff-only", "master")
        self.run_git("merge", "-q", "--no-ff", "master", "-m", "Merge stable backflow")
        self.run_git("merge-base", "--is-ancestor", feature, "dev")
        self.run_git("merge-base", "--is-ancestor", hotfix, "dev")
        self.assertEqual((self.root / "feature.txt").read_text(), "feature\n")
        self.assertEqual((self.root / "hotfix.txt").read_text(), "hotfix\n")

    def test_invalid_json_yaml_cli_exits_nonzero(self):
        self.write(".github/ISSUE_TEMPLATE/config.yml", '{"blank_issues_enabled": false\n')
        result = subprocess.run(
            [sys.executable, "-B", str(self.root / "scripts/check-repo.py")],
            capture_output=True, text=True, timeout=10,
        )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("invalid JSON", result.stderr)


if __name__ == "__main__":
    unittest.main()
