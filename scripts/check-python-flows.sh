#!/usr/bin/env bash
#
# The lane's boundary, read from its import graph. Adapted from
# galata-datawatch's guard of the same name; it reads with `ast`, so a name in
# a comment is not an import and an import split across lines is still one.
#
#   1. IMPORTS ARE A SHORT LIST: cereyan, and the stdlib a flow needs to
#      read lane.toml, parse what a tool printed and spawn a tool. `json` is
#      the one addition to datawatch's list: the poll prints alerts as JSON
#      lines, and parsing text reaches neither the network nor the
#      environment. Anything else, `os` included (`os.environ` is how an
#      environment gets inherited), is a diff that has to argue for itself.
#   2. ONE MODULE SPAWNS. Only `_runner.py` imports `subprocess`.
#   3. EVERY SPAWN BUILDS ITS ENVIRONMENT. A `subprocess.run` or `Popen`
#      without `env=` inherits the scheduler's.
#   4. NO CREDENTIAL IS NAMED: PASSWORD, SECRET, a *_TOKEN or a `_KEY` in an
#      identifier or a string. galata-notify reads its own token file.
#
# Docstrings are exempt from 4: they say why the rule exists.
#
# Usage: check-python-flows.sh [flows-dir]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "${1:-$ROOT/lane/flows}" <<'PY'
import ast, pathlib, re, sys
flows = pathlib.Path(sys.argv[1])
ALLOWED = {"cereyan", "subprocess", "pathlib", "datetime", "tomllib", "json", "__future__", "typing"}
SPAWNER = "_runner.py"
CREDENTIAL = re.compile(r"PASSWORD|SECRET|[A-Z]+_TOKEN|_KEY\b")
def docstrings(tree):
    ids = set()
    for node in ast.walk(tree):
        if isinstance(node, (ast.Module, ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            body = node.body
            if body and isinstance(body[0], ast.Expr) and isinstance(body[0].value, ast.Constant) \
                    and isinstance(body[0].value.value, str):
                ids.add(id(body[0].value))
    return ids
problems = []
files = sorted(flows.glob("*.py"))
if not files:
    problems.append(f"{flows}: no flows to check; a guard over nothing is green for the wrong reason")
for path in files:
    rel = path.relative_to(flows.parent)
    tree = ast.parse(path.read_text(), filename=str(path))
    docs = docstrings(tree)
    for node in ast.walk(tree):
        at = f"{rel}:{getattr(node, 'lineno', '?')}"
        if isinstance(node, ast.Import):
            roots = [a.name.split(".")[0] for a in node.names]
        elif isinstance(node, ast.ImportFrom):
            roots = [] if node.level else [(node.module or "").split(".")[0]]
        else:
            roots = []
        for root in roots:
            if root not in ALLOWED:
                problems.append(f"{at}: imports {root!r}, which the lane does not allow")
            if root == "subprocess" and path.name != SPAWNER:
                problems.append(f"{at}: imports subprocess outside {SPAWNER}")
        if isinstance(node, ast.Call):
            f = node.func
            name = f.attr if isinstance(f, ast.Attribute) else getattr(f, "id", "")
            if name in {"run", "Popen", "call", "check_call", "check_output"} and \
                    isinstance(f, ast.Attribute) and getattr(f.value, "id", "") == "subprocess":
                if not any(k.arg == "env" for k in node.keywords):
                    problems.append(f"{at}: subprocess.{name} without env= inherits the scheduler's environment")
        if isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in docs:
            if CREDENTIAL.search(node.value):
                problems.append(f"{at}: names a credential in {node.value!r}")
        if isinstance(node, ast.Name) and CREDENTIAL.search(node.id):
            problems.append(f"{at}: names a credential: {node.id}")
        if isinstance(node, ast.Attribute) and CREDENTIAL.search(node.attr):
            problems.append(f"{at}: names a credential: {node.attr}")
if problems:
    print("check-python-flows:", file=sys.stderr)
    for p in problems:
        print(f"  {p}", file=sys.stderr)
    sys.exit(1)
print(f"python flows: ok. {len(files)} modules; one spawner, a built environment, no credential")
PY
