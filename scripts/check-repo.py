#!/usr/bin/env python3
"""Read-only repository checks, using Python's standard library and Git."""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote, urlsplit


REQUIRED_FILES = (
    "README.md", "AGENTS.md", "CLAUDE.md", "CONTRIBUTING.md", "LICENSE",
    "SECURITY.md", "CODE_OF_CONDUCT.md", ".editorconfig", ".gitattributes",
    ".gitignore", "version.json", "docs/README.md", "docs/status/current.md",
    "docs/development/README.md", "docs/governance/README.md",
    "docs/governance/agent-collaboration.md",
    "docs/governance/repository-governance.md",
    "docs/adr/0002-repository-governance.md", ".github/pull_request_template.md",
    ".github/repository-settings.json", ".github/rulesets/README.md",
    ".github/rulesets/master-protection.json", ".github/workflows/pr-check.yml",
    ".github/ISSUE_TEMPLATE/config.yml", ".github/ISSUE_TEMPLATE/bug-report.yml",
    ".github/ISSUE_TEMPLATE/proposal.yml", "scripts/README.md",
    "scripts/check-repo.py", "scripts/check-repo.sh", "scripts/check-docs.sh",
    "scripts/check-text-files.sh", "scripts/tests/test_repository_checks.py",
)
TEXT_SUFFIXES = {
    ".md", ".txt", ".json", ".jsonc", ".yaml", ".yml", ".toml", ".ini",
    ".cfg", ".sh", ".py", ".rs", ".go", ".dart", ".sql", ".csv", ".tsv",
    ".xml", ".svg", ".html", ".css", ".js", ".ts", ".lock", ".mod",
    ".sum", ".gradle", ".properties", ".kt", ".swift", ".h", ".c", ".cpp",
}
BINARY_SUFFIXES = {
    ".png", ".jpg", ".jpeg", ".gif", ".webp", ".ico", ".pdf", ".zip",
    ".gz", ".7z", ".woff", ".woff2", ".ttf", ".otf", ".apk", ".aab",
    ".ipa", ".dylib", ".so", ".a", ".sqlite", ".db", ".ledger-backup",
}
JSON_YAML_FILES = {
    ".github/workflows/pr-check.yml", ".github/ISSUE_TEMPLATE/config.yml",
    ".github/ISSUE_TEMPLATE/bug-report.yml", ".github/ISSUE_TEMPLATE/proposal.yml",
}
MAX_FILE_BYTES = 5 * 1024 * 1024
AGGREGATE_RUN = """python3 - <<'PY'
import json, os, sys
results = json.loads(os.environ['NEEDS_JSON'])
for name, result in results.items():
    print(name + ': ' + result.get('result', 'missing'))
sys.exit(0 if results and all(item.get('result') == 'success' for item in results.values()) else 1)
PY"""
CHECK_RUN = """if [[ -n "${PR_BASE_SHA}" ]]; then
  ./scripts/check-repo.sh --base-ref "${PR_BASE_SHA}" --head-ref "${PR_HEAD_SHA}"
else
  ./scripts/check-repo.sh
fi"""
COMMIT_SUBJECT = re.compile(
    r"^(feat|fix|docs|style|refactor|perf|test|build|ci|chore|revert)"
    r"(?:\([^()\r\n]+\))?!?: \S.*$"
)
AI_AUTHOR = re.compile(
    r"^Co-authored-by:.*\b(codex|copilot|chatgpt|openai|claude|anthropic|gpt|gemini)\b",
    re.IGNORECASE | re.MULTILINE,
)


def git(root, *args):
    result = subprocess.run(
        ["git", "-C", str(root), *args], capture_output=True, timeout=30,
    )
    if result.returncode:
        detail = result.stderr.decode("utf-8", errors="replace").strip()
        raise ValueError(f"git {args[0]} failed: {detail or 'nonzero exit'}")
    return result.stdout


def repository_files(root):
    raw = git(root, "ls-files", "--cached", "--others", "--exclude-standard", "-z")
    files = []
    for name in sorted(set(raw.decode("utf-8").split("\0")) - {""}):
        path = root / name
        if not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"path escapes repository: {name}")
        if path.is_symlink():
            raise ValueError(f"symlink needs an explicit repository policy: {name}")
        if path.is_file():
            files.append(name)
    return files


def unique_object(pairs):
    result = {}
    for key, value in pairs:
        if key in result:
            raise ValueError(f"duplicate JSON key: {key}")
        result[key] = value
    return result


def reject_json_constant(value):
    raise ValueError(f"nonstandard JSON constant: {value}")


def ignored_tracked_files(root):
    tracked = git(root, "ls-files", "--cached", "-z")
    if not tracked:
        return []
    result = subprocess.run(
        ["git", "-C", str(root), "check-ignore", "--no-index", "--stdin", "-z"],
        input=tracked, capture_output=True, timeout=30,
    )
    if result.returncode not in (0, 1):
        raise ValueError("git check-ignore failed while checking tracked artifacts")
    return [name for name in result.stdout.decode("utf-8").split("\0") if name]


def check_text(root, files):
    errors, texts, documents = [], {}, {}
    for name in files:
        path = root / name
        if path.stat().st_size > MAX_FILE_BYTES:
            errors.append(f"{name}: file exceeds 5 MiB; review artifact policy")
            continue
        if path.suffix.lower() in BINARY_SUFFIXES:
            continue
        raw = path.read_bytes()
        known_text = path.suffix.lower() in TEXT_SUFFIXES or not path.suffix
        if b"\0" in raw:
            if known_text:
                errors.append(f"{name}: NUL in a text file")
            continue
        try:
            text = raw.decode("utf-8")
        except UnicodeDecodeError:
            if known_text:
                errors.append(f"{name}: text is not UTF-8")
            continue
        texts[name] = text
        if raw.startswith(b"\xef\xbb\xbf"):
            errors.append(f"{name}: UTF-8 BOM")
        if b"\r" in raw:
            errors.append(f"{name}: CR / CRLF, expected LF")
        if raw and not raw.endswith(b"\n"):
            errors.append(f"{name}: missing final newline")
        for number, line in enumerate(text.splitlines(), 1):
            trailing = re.search(r"[ \t]+$", line)
            if trailing and not (
                path.suffix == ".md" and trailing.group() == "  "
                and line[:trailing.start()].strip()
            ):
                errors.append(f"{name}:{number}: trailing whitespace")
        if path.suffix == ".json" or name in JSON_YAML_FILES:
            try:
                documents[name] = json.loads(
                    text, object_pairs_hook=unique_object,
                    parse_constant=reject_json_constant,
                )
            except (ValueError, json.JSONDecodeError) as error:
                errors.append(f"{name}: invalid JSON / JSON-compatible YAML: {error}")
    return errors, texts, documents


def markdown_content(text):
    """Remove code fences without treating their paths or headings as prose."""
    lines, fence = [], None
    for line in text.splitlines():
        match = re.match(r"^\s{0,3}(`{3,}|~{3,})(.*)$", line)
        if fence:
            if match and match[1][0] == fence[0] and len(match[1]) >= fence[1] and not match[2].strip():
                fence = None
            lines.append("")
        elif match:
            fence = (match[1][0], len(match[1]))
            lines.append("")
        else:
            lines.append(line)
    return "\n".join(lines), fence is not None


def markdown_anchors(content):
    anchors, counts = set(), {}
    for line in content.splitlines():
        match = re.match(r"^\s{0,3}#{1,6}\s+(.+?)\s*#*\s*$", line)
        if not match:
            continue
        heading = re.sub(r"<[^>]+>", "", match[1]).lower()
        heading = re.sub(r"[^\w\- ]", "", heading).replace(" ", "-")
        occurrence = counts.get(heading, 0)
        counts[heading] = occurrence + 1
        anchors.add(heading if not occurrence else f"{heading}-{occurrence}")
    return anchors


def table_cells(line):
    line = re.sub(r"`[^`]*`", lambda m: m[0].replace("|", r"\|"), line)
    return re.split(r"(?<!\\)\|", line.strip().strip("|"))


def check_docs(root, files, texts):
    errors, bodies, anchors = [], {}, {}
    for name, text in texts.items():
        if not name.endswith(".md"):
            continue
        body, unclosed = markdown_content(text)
        bodies[name] = body
        anchors[name] = markdown_anchors(body)
        if unclosed:
            errors.append(f"{name}: unclosed code fence")
        lines = body.splitlines()
        for index, line in enumerate(lines):
            if "|" not in line or not all(
                re.fullmatch(r"\s*:?-{3,}:?\s*", cell) for cell in table_cells(line)
            ):
                continue
            width = len(table_cells(line))
            if index == 0 or len(table_cells(lines[index - 1])) != width:
                errors.append(f"{name}:{index + 1}: table header width differs")
            for row in range(index + 1, len(lines)):
                if not lines[row].strip() or "|" not in lines[row]:
                    break
                if len(table_cells(lines[row])) != width:
                    errors.append(f"{name}:{row + 1}: table row width differs")
    file_set = set(files)
    for name, body in bodies.items():
        # Current documentation uses inline links or reference definitions.
        links = re.findall(r"\[[^\]\n]*\]\((<[^>\n]+>|[^\s)]+)(?:\s+\"[^\"]*\")?\)", body)
        definitions = dict(re.findall(r"^\s*\[([^\]\n]+)\]:\s*(\S+)", body, re.MULTILINE))
        links.extend(definitions.values())
        for label, ref in re.findall(r"\[([^\]\n]+)\]\[([^\]\n]*)\]", body):
            if (ref or label) not in definitions:
                errors.append(f"{name}: undefined reference link: {ref or label}")
        for link in links:
            parsed = urlsplit(link.strip("<>"))
            if parsed.scheme or parsed.netloc:
                continue
            link_path = unquote(parsed.path)
            target = (root / name).parent / link_path if link_path else root / name
            target = target.resolve()
            if not target.is_relative_to(root.resolve()):
                errors.append(f"{name}: local link escapes repository: {link}")
                continue
            relative = target.relative_to(root.resolve()).as_posix()
            present = relative in file_set or (
                target.is_dir() and any(p.startswith(relative + "/") for p in files)
            )
            if not present:
                errors.append(f"{name}: missing or ignored local link: {link}")
            elif parsed.fragment and relative.endswith(".md"):
                if unquote(parsed.fragment) not in anchors.get(relative, set()):
                    errors.append(f"{name}: missing Markdown anchor: {link}")
    for name in ("AGENTS.md", "CLAUDE.md"):
        if name not in texts:
            errors.append(f"{name}: missing readable root collaboration entry")
    if texts.get("AGENTS.md") != texts.get("CLAUDE.md"):
        errors.append("AGENTS.md / CLAUDE.md must be identical")
    return errors


def check_governance(root, files, documents):
    errors = []

    def require(condition, message):
        if not condition:
            errors.append(message)

    def object_at(name):
        value = documents.get(name)
        require(isinstance(value, dict), f"{name}: expected a valid JSON object")
        return value if isinstance(value, dict) else {}

    for name in REQUIRED_FILES:
        require(name in files, f"missing required asset: {name}")
    for name in ignored_tracked_files(root):
        errors.append(f"ignored artifact is tracked: {name}; remove from index under task authorization")
    for name in ("check-repo.sh", "check-docs.sh", "check-text-files.sh"):
        path = root / "scripts" / name
        require(path.is_file() and os.access(path, os.X_OK), f"scripts/{name}: not executable")

    version = object_at("version.json")
    require(type(version.get("schemaVersion")) is int and version["schemaVersion"] == 1, "version.json: unsupported schemaVersion")
    require(
        bool(re.fullmatch(
            r"\d{2}\.(?:[1-9]|1[0-2])\.[1-9]\d*",
            str(version.get("productVersion", "")),
        )),
        "version.json: expected YY.M.RELEASE",
    )
    build = version.get("flutterBuildNumber")
    require(type(build) is int and build > 0, "version.json: build number must be a positive integer")

    settings = object_at(".github/repository-settings.json")
    require(settings == {
        "default_branch": "master", "allow_merge_commit": True,
        "allow_rebase_merge": True, "allow_squash_merge": False,
        "allow_auto_merge": False, "delete_branch_on_merge": False,
    }, "repository settings drift: default branch / merge / deletion contract")

    policy = object_at(".github/rulesets/master-protection.json")
    require(policy.get("target") == "branch" and policy.get("enforcement") == "active", "Ruleset: expected active branch target")
    require(policy.get("conditions") == {"ref_name": {"include": ["refs/heads/master"], "exclude": []}}, "Ruleset: must match master only")
    require(
        policy.get("bypass_actors") == [{
            "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request",
        }],
        "Ruleset: only PR-scoped admin bypass is allowed",
    )
    rules = policy.get("rules", [])
    if not isinstance(rules, list) or not all(
        isinstance(item, dict) and isinstance(item.get("type"), str) for item in rules
    ):
        errors.append("Ruleset: invalid rules list")
        rules = []
    by_type = {rule.get("type"): rule for rule in rules}
    require(
        len(rules) == len(by_type) and set(by_type) == {
            "deletion", "non_fast_forward", "pull_request", "required_status_checks",
        },
        "Ruleset: protection types differ from reviewed contract",
    )
    require(by_type.get("pull_request", {}).get("parameters") == {
        "allowed_merge_methods": ["merge", "rebase"],
        "dismiss_stale_reviews_on_push": True, "require_code_owner_review": False,
        "require_last_push_approval": False, "required_approving_review_count": 0,
        "required_review_thread_resolution": True,
    }, "Ruleset: PR merge / review contract drift")
    status = by_type.get("required_status_checks", {}).get("parameters", {})
    if not isinstance(status, dict):
        status = {}
    require(
        status.get("strict_required_status_checks_policy") is True
        and status.get("do_not_enforce_on_create") is False,
        "Ruleset: strict checks and creation enforcement required",
    )
    contexts = status.get("required_status_checks", [])
    require(
        isinstance(contexts, list) and len(contexts) == 1
        and isinstance(contexts[0], dict) and contexts[0].get("context") == "Candidate Quality",
        "Ruleset: required context must be Candidate Quality only",
    )

    workflow = object_at(".github/workflows/pr-check.yml")
    workflow_files = {
        name for name in files if name.startswith(".github/workflows/")
        and Path(name).suffix in {".yml", ".yaml"}
    }
    require(
        workflow_files == {".github/workflows/pr-check.yml"},
        "workflow inventory changed; review triggers and required-context uniqueness",
    )
    events = workflow.get("on", {})
    require(
        isinstance(events, dict) and set(events) == {"pull_request", "workflow_dispatch"},
        "workflow: only PR and manual triggers allowed",
    )
    pr_event = events.get("pull_request", {}) if isinstance(events, dict) else {}
    require(
        isinstance(pr_event, dict) and pr_event.get("branches") == ["dev", "master"]
        and "paths" not in pr_event and "paths-ignore" not in pr_event,
        "workflow: unfiltered dev/master PR checks required",
    )
    require(workflow.get("permissions") == {"contents": "read"}, "workflow: contents must be read-only")
    jobs = workflow.get("jobs", {})
    if not isinstance(jobs, dict) or not all(isinstance(job, dict) for job in jobs.values()):
        errors.append("workflow: invalid jobs object")
        jobs = {}
    require(set(jobs) == {"repo-hygiene", "candidate-quality"}, "workflow: update the reviewed component contract when adding jobs")
    for job_id, job in jobs.items():
        timeout = job.get("timeout-minutes")
        require(type(timeout) is int and 0 < timeout <= 20, f"workflow {job_id}: finite timeout required")
        require("continue-on-error" not in job and "permissions" not in job, f"workflow {job_id}: no error masking or permission overrides")
        steps = job.get("steps", [])
        require(
            isinstance(steps, list) and all(
                isinstance(step, dict) and "continue-on-error" not in step and "if" not in step
                for step in steps
            ),
            f"workflow {job_id}: steps cannot mask failures or skip checks",
        )
    hygiene = jobs.get("repo-hygiene", {})
    require(hygiene.get("name") == "Repo Hygiene" and "if" not in hygiene, "workflow: Repo Hygiene cannot be skipped")
    steps = hygiene.get("steps", [])
    if isinstance(steps, list) and len(steps) == 2 and all(isinstance(s, dict) for s in steps):
        checkout, check = steps
        require(
            bool(re.fullmatch(r"actions/checkout@[0-9a-f]{40}", str(checkout.get("uses", "")))),
            "workflow: checkout must use full commit SHA",
        )
        require(
            checkout.get("with") == {"fetch-depth": 0, "persist-credentials": False},
            "workflow: candidate checkout must have complete history and no credentials",
        )
        require(
            check.get("env") == {
                "PR_BASE_SHA": "${{ github.event.pull_request.base.sha }}",
                "PR_HEAD_SHA": "${{ github.event.pull_request.head.sha }}",
            } and check.get("run") == CHECK_RUN,
            "workflow: full check and PR range must be passed through environment",
        )
    else:
        errors.append("workflow: expected candidate checkout and full repository check")
    candidate = jobs.get("candidate-quality", {})
    require(
        candidate.get("name") == "Candidate Quality" and candidate.get("if") == "always()"
        and candidate.get("needs") == ["repo-hygiene"],
        "workflow: aggregate name / always / dependencies drift",
    )
    summary = candidate.get("steps", [])
    require(
        isinstance(summary, list) and len(summary) == 1 and isinstance(summary[0], dict)
        and summary[0].get("env") == {"NEEDS_JSON": "${{ toJSON(needs) }}"}
        and summary[0].get("run") == AGGREGATE_RUN,
        "workflow: aggregate must reject every non-success component",
    )

    issue_config = object_at(".github/ISSUE_TEMPLATE/config.yml")
    contacts = issue_config.get("contact_links")
    require(
        issue_config.get("blank_issues_enabled") is False
        and isinstance(contacts, list)
        and any(isinstance(item, dict) and item.get("url") == "mailto:luobo@radishx.com" for item in contacts),
        "Issues: private security route and structured reports required",
    )
    for name in ("bug-report", "proposal"):
        form = object_at(f".github/ISSUE_TEMPLATE/{name}.yml")
        body = form.get("body", [])
        require(
            isinstance(body, list) and bool(body)
            and all(isinstance(item, dict) and isinstance(item.get("id"), str) for item in body),
            f"Issue {name}: structured fields required",
        )
        if isinstance(body, list) and all(isinstance(item, dict) and isinstance(item.get("id"), str) for item in body):
            ids = [item.get("id") for item in body]
            require(len(ids) == len(set(ids)), f"Issue {name}: duplicate field IDs")
    sources = [
        name for name in files if Path(name).suffix in {".rs", ".go", ".dart"}
        or Path(name).name in {"Cargo.toml", "go.mod", "pubspec.yaml"}
    ]
    require(
        not sources,
        "language implementation has no quality component; add real checks before introducing: "
        + ", ".join(sources[:5]),
    )
    return errors


def commit_errors(root, base_ref, head_ref):
    base = git(root, "rev-parse", "--verify", "--end-of-options", base_ref + "^{commit}").decode().strip()
    head = git(root, "rev-parse", "--verify", "--end-of-options", head_ref + "^{commit}").decode().strip()
    common = git(root, "merge-base", base, head).decode().strip()
    commits = git(root, "rev-list", "--reverse", "--parents", f"{common}..{head}").decode().splitlines()
    errors = []
    for commit in commits:
        sha, *parents = commit.split()
        message = git(root, "show", "-s", "--format=%B", sha).decode("utf-8")
        subject = message.splitlines()[0] if message.splitlines() else ""
        if len(parents) <= 1 and not COMMIT_SUBJECT.fullmatch(subject):
            errors.append(f"commit {sha[:12]}: expected Conventional Commits subject")
        if AI_AUTHOR.search(message):
            errors.append(f"commit {sha[:12]}: AI co-author attribution is not allowed")
    # Inspect the actual candidate tree, including conflict resolutions.
    git(root, "diff", "--check", base, "HEAD")
    return errors


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--only", choices=("text", "docs", "repo"), default="repo")
    parser.add_argument("--base-ref")
    parser.add_argument("--head-ref")
    args = parser.parse_args()
    if args.head_ref and not args.base_ref:
        parser.error("--head-ref requires --base-ref")
    if args.only != "repo" and args.base_ref:
        parser.error("PR commit range requires the complete repository check")
    root = Path(__file__).resolve().parents[1]
    errors = []
    try:
        files = repository_files(root)
        errors, texts, documents = check_text(root, files)
        if args.only != "text":
            errors.extend(check_docs(root, files, texts))
        if args.only == "repo":
            errors.extend(check_governance(root, files, documents))
            git(root, "diff", "--check")
            git(root, "diff", "--cached", "--check")
            if args.base_ref:
                errors.extend(commit_errors(root, args.base_ref, args.head_ref or "HEAD"))
            if not errors:
                result = subprocess.run(
                    [sys.executable, "-B", "-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py"],
                    cwd=root, timeout=60,
                )
                if result.returncode:
                    errors.append("repository checker regression tests failed")
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        errors.append(str(error))
    for error in errors:
        print("ERROR: " + error, file=sys.stderr)
    if errors:
        return 1
    print(f"{args.only}: checked {len(files)} repository files; passed")
    if args.only == "repo" and not args.base_ref:
        print("No PR commit range supplied; history was not checked.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
