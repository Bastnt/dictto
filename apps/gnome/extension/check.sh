#!/usr/bin/env bash
# Static checks for the Shell extension where no GNOME Shell runs (WSL, CI).
# gi:// and resource:/// imports only resolve inside gnome-shell, so the modules are
# not executed: SpiderMonkey's Reflect.parse (exposed by gjs) parses each one as a
# module, which catches syntax errors, and its AST gives the relative imports to check.
# Needs gjs and jq; the icon is parsed with xmllint, or python3 when xmllint is absent.
set -euo pipefail
cd "$(dirname "$0")"

fail() { echo "FAIL: $*" >&2; exit 1; }

echo "metadata.json"
jq -e '
  .uuid == "dictto@dictto.app"
  and (.name | type == "string" and length > 0)
  and (.description | type == "string" and length > 0)
  and .["shell-version"] == ["47", "48", "49"]
' metadata.json >/dev/null || fail "metadata.json: uuid, name, description or shell-version"

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat >"$work/parse.js" <<'EOF'
import GLib from 'gi://GLib';
import System from 'system';

let failed = false;
for (const path of System.programArgs) {
    const [, bytes] = GLib.file_get_contents(path);
    let ast;
    try {
        ast = Reflect.parse(new TextDecoder().decode(bytes), {target: 'module', source: path});
    } catch (error) {
        printerr(`${path}: ${error}`);
        failed = true;
        continue;
    }
    for (const node of ast.body) {
        if (node.type !== 'ImportDeclaration')
            continue;
        const specifier = node.moduleRequest?.source?.value ?? node.source?.value;
        if (typeof specifier !== 'string') {
            printerr(`${path}: cannot read an import specifier`);
            failed = true;
        } else if (specifier.startsWith('.')) {
            const target = GLib.build_filenamev([GLib.path_get_dirname(path), specifier]);
            if (!GLib.file_test(target, GLib.FileTest.EXISTS)) {
                printerr(`${path}: missing import ${specifier}`);
                failed = true;
            }
        } else if (!/^(gi:\/\/|resource:\/\/\/org\/gnome\/shell\/|system$|cairo$)/.test(specifier)) {
            printerr(`${path}: unexpected import ${specifier}`);
            failed = true;
        }
    }
}
if (failed)
    System.exit(1);
EOF

echo "module syntax and imports"
gjs -m "$work/parse.js" ./*.js || fail "module parse"

echo "embedded D-Bus XML"
cat >"$work/xml.js" <<EOF
import Gio from 'gi://Gio';
import {APP_XML, SHELL_XML} from 'file://$PWD/dbus.js';
for (const xml of [APP_XML, SHELL_XML])
    Gio.DBusNodeInfo.new_for_xml(xml);
for (const file of ['com.dictto.App1.xml', 'com.dictto.Shell1.xml']) {
    const [, bytes] = Gio.File.new_for_path('$PWD/../dbus/' + file).load_contents(null);
    Gio.DBusNodeInfo.new_for_xml(new TextDecoder().decode(bytes));
}
EOF
gjs -m "$work/xml.js" || fail "D-Bus XML"

echo "icon"
if command -v xmllint >/dev/null; then
    xmllint --noout icons/dictto-symbolic.svg || fail "icon XML"
elif command -v python3 >/dev/null; then
    python3 -I -c 'import sys, xml.dom.minidom as m; assert m.parse(sys.argv[1]).documentElement.tagName == "svg"' \
        icons/dictto-symbolic.svg || fail "icon XML"
else
    fail "icon: neither xmllint nor python3 to parse it"
fi

echo "check.sh: PASS"
