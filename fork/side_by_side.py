#!/usr/bin/env python3
"""Turns an (upstream-named) rakuyomi.koplugin into the side-by-side fork.

KOReader runs every plugin in one Lua state: `package.loaded`, the ffi C
namespace, `G_reader_settings`, dispatcher actions and main-menu keys are all
shared. The upstream sources in `frontend/rakuyomi.koplugin` are kept untouched
(apart from small, generic fixes) so the fork rebases cleanly; everything that
makes the fork coexist with the original rakuyomi is applied here, at package
time, driven by `fork/side-by-side.json`:

* plugin folder / `_meta.lua` name, menu entry, dispatcher action + event;
* modules are required as `<module_prefix><path>` and resolved by a package
  searcher injected into `main.lua`;
* `rakuyomi_*` settings keys, files and ffi names -> `<settings_prefix>*`;
* data dir, Unix socket (passed to the server via RAKUYOMI_UNIX_SOCKET_PATH);
* CBZ provider priority, disabled self-update (it would install upstream);
* `fork/lua/BundledSources.lua` + hook that installs `bundled-sources/*.aix`.

Every rule must match; if upstream changes the code a rule targets, the build
fails loudly instead of silently producing a plugin that clashes.

Usage: side_by_side.py SRC_PLUGIN_DIR DST_PLUGIN_DIR [--config FILE]
"""
import argparse
import json
import os
import re
import shutil
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


class RuleError(Exception):
    pass


def sub_must(pattern, repl, text, what, count=0, flags=0):
    if isinstance(repl, str):
        literal = repl
        repl = lambda _m: literal  # noqa: E731 - never interpret escapes
    new, n = re.subn(pattern, repl, text, count=count, flags=flags)
    if n == 0:
        raise RuleError(f"rule did not match: {what}")
    return new


def lua_str(s):
    return json.dumps(s, ensure_ascii=False)


def module_names(root):
    names = set()
    for dirpath, _dirs, files in os.walk(root):
        for f in files:
            if f.endswith(".lua"):
                rel = os.path.relpath(os.path.join(dirpath, f), root)
                names.add(rel[:-4].replace(os.sep, "/"))
    return names


SEARCHER = """-- side-by-side packaging (fork/side_by_side.py): KOReader shares
-- `package.loaded` between plugins, so this plugin's modules are required as
-- `{prefix}<path>` and resolved from this plugin's folder by this searcher.
do
  local plugin_dir = debug.getinfo(1, "S").source:match("^@(.*)/[^/]*$") or "."
  local prefix = {prefix_lua}
  local searchers = package.loaders or package.searchers
  if not package.loaded[prefix .. "__searcher"] then
    package.loaded[prefix .. "__searcher"] = true
    table.insert(searchers, 2, function(name)
      if name:sub(1, #prefix) ~= prefix then
        return nil
      end
      local path = plugin_dir .. "/" .. name:sub(#prefix + 1) .. ".lua"
      local chunk, err = loadfile(path)
      if not chunk then
        return "\\n\\tno file '" .. path .. "' (" .. tostring(err) .. ")"
      end
      return chunk
    end)
  end
end

"""


def transform(src, dst, cfg):
    if os.path.exists(dst):
        shutil.rmtree(dst)
    shutil.copytree(src, dst, symlinks=False)
    shutil.copy(os.path.join(HERE, "lua", "BundledSources.lua"), os.path.join(dst, "BundledSources.lua"))

    modules = module_names(dst)
    prefix = cfg["module_prefix"]
    sprefix = cfg["settings_prefix"]
    req_re = re.compile(r"""require(\s*\(?\s*)(["'])([^"']+)\2""")

    def fix_require(m):
        name = m.group(3)
        if name in modules:
            return f"require{m.group(1)}{m.group(2)}{prefix}{name}{m.group(2)}"
        return m.group(0)

    def path(rel):
        return os.path.join(dst, *rel.split("/"))

    def read(rel):
        with open(path(rel), encoding="utf-8", newline="") as f:
            return f.read()

    def write(rel, text):
        with open(path(rel), "w", encoding="utf-8", newline="") as f:
            f.write(text)

    # 1. Generic rules over every Lua file (translations in l10n/ untouched).
    for mod in sorted(modules):
        rel = mod + ".lua"
        if rel.startswith("l10n/"):
            continue
        t = read(rel)
        t = req_re.sub(fix_require, t)
        # settings keys, setting files, ffi names, ui ids: rakuyomi_* -> prefix
        t = re.sub(r"rakuyomi_(?!bridge)", sprefix, t)
        for key in cfg["extra_settings_keys"]:
            t = re.sub(r"""(["'])%s\1""" % re.escape(key), lambda m: f"{m.group(1)}{sprefix}{key}{m.group(1)}", t)
        t = t.replace("go_back_to_rakuyomi", "go_back_to_" + cfg["plugin_name"])
        write(rel, t)

    fullname = cfg["fullname"]

    # 2. Targeted rules.
    t = read("main.lua")
    t = SEARCHER.format(prefix=prefix, prefix_lua=lua_str(prefix)) + t
    t = sub_must(r'name = "rakuyomi"', f'name = {lua_str(cfg["plugin_name"])}', t, "main.lua plugin name")
    t = sub_must(r'"start_library_view"', lua_str(sprefix + "start_library_view"), t, "dispatcher action")
    t = sub_must(r"StartLibraryView", cfg["library_event"], t, "library event")
    t = sub_must(r"menu_items\.rakuyomi\b", "menu_items." + cfg["plugin_name"], t, "main menu key")
    t = sub_must(r'_\("Rakuyomi"\)', lua_str(fullname), t, "menu/action title")
    t = sub_must(r'_\("Oops!"\) \.\. ', '_("Oops!") .. %s .. ' % lua_str(fullname + ": "), t, "startup error prefix")
    write("main.lua", t)

    write("_meta.lua", "-- generated by fork/side_by_side.py\nreturn {\n  name = %s,\n  fullname = %s,\n  description = %s,\n}\n"
          % (lua_str(cfg["plugin_name"]), lua_str(fullname), lua_str(cfg["description"])))

    t = read("MangaReader.lua")
    t = sub_must(r'_\("Go back to Rakuyomi\.\.\."\)', lua_str(f"Go back to {fullname}..."), t, "go back menu text")
    write("MangaReader.lua", t)

    t = read("gettext+.lua")
    t = sub_must(r'require\("datastorage"\):getFullDataDir\(\) \.\. "/plugins/rakuyomi\.koplugin/l10n"',
                 'require(%s).getPluginDirectory() .. "/l10n"' % lua_str(prefix + "Paths"), t, "l10n dir")
    write("gettext+.lua", t)

    t = read("Paths.lua")
    t = sub_must(r'"/rakuyomi"', lua_str("/" + cfg["data_dir"]), t, "data dir")
    write("Paths.lua", t)

    rel = "platform/generic_unix_platform.lua"
    t = read(rel)
    t = sub_must(r"local SOCKET_PATH = '/tmp/rakuyomi\.sock'", "local SOCKET_PATH = " + lua_str(cfg["socket_path"]), t, "socket path")
    t = sub_must(r"(\nlocal SOCKET_PATH = [^\n]*\n)",
                 lambda m: m.group(1) + "pcall(ffi.cdef, [[ int setenv(const char *name, const char *value, int overwrite);"
                 " int unsetenv(const char *name); ]])\n",
                 t, "setenv cdef", count=1)
    t = sub_must(r"(\n(\s*)local spawn_res = C\.posix_spawn\([^\n]*\n)",
                 lambda m: (
                     f"\n{m.group(2)}-- side-by-side: the server listens on this plugin's own socket\n"
                     f"{m.group(2)}local prev_socket_env = os.getenv('RAKUYOMI_UNIX_SOCKET_PATH')\n"
                     f"{m.group(2)}C.setenv('RAKUYOMI_UNIX_SOCKET_PATH', SOCKET_PATH, 1)"
                     f"{m.group(1)}"
                     f"{m.group(2)}if prev_socket_env ~= nil then C.setenv('RAKUYOMI_UNIX_SOCKET_PATH', prev_socket_env, 1)"
                     f" else C.unsetenv('RAKUYOMI_UNIX_SOCKET_PATH') end\n"),
                 t, "server spawn env", count=1)
    t = sub_must(r"(\nfunction GenericUnixPlatform:startServer\(\)\n)",
                 lambda m: m.group(1) + (
                     f"  local bundled_ok, bundled_err = pcall(function() require({lua_str(prefix + 'BundledSources')}).install() end)\n"
                     "  if not bundled_ok then logger.warn(\"failed to install bundled sources\", bundled_err) end\n"),
                 t, "bundled sources hook", count=1)
    write(rel, t)

    rel = "extensions/CbzDocument.lua"
    t = read(rel)
    t = sub_must(r'(registry:addProvider\("cbz", "application/vnd\.comicbook\+zip", self, )\d+\)',
                 lambda m: f"{m.group(1)}{cfg['cbz_provider_priority']})", t, "cbz provider priority")
    write(rel, t)

    if cfg.get("disable_self_update"):
        t = read("UpdateChecker.lua")
        t = sub_must(r"\nreturn UpdateChecker\s*$",
                     "\n-- side-by-side: the built-in updater installs upstream releases, which\n"
                     "-- would replace this fork, so it is disabled.\n"
                     "function UpdateChecker:checkForUpdates()\n"
                     f"  UIManager:show(InfoMessage:new {{ text = {lua_str(cfg['self_update_message'])} }})\n"
                     "end\n\nreturn UpdateChecker\n", t, "self update")
        write("UpdateChecker.lua", t)

    # 3. Sanity checks: nothing may still point at upstream names.
    problems = []
    for mod in sorted(module_names(dst)):
        rel = mod + ".lua"
        if rel.startswith("l10n/"):
            continue
        t = read(rel)
        for m in req_re.finditer(t):
            if m.group(3) in modules:
                problems.append(f"{rel}: bare require {m.group(3)}")
        for m in re.finditer(r"rakuyomi_(?!bridge)|/tmp/rakuyomi\.sock|\"/rakuyomi\"|rakuyomi\.koplugin", t):
            problems.append(f"{rel}: leftover {m.group(0)}")
    if problems:
        raise RuleError("leftover upstream names:\n  " + "\n  ".join(problems))


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("src")
    ap.add_argument("dst")
    ap.add_argument("--config", default=os.path.join(HERE, "side-by-side.json"))
    args = ap.parse_args()
    with open(args.config, encoding="utf-8") as f:
        cfg = json.load(f)
    try:
        transform(args.src, args.dst, cfg)
    except RuleError as e:
        print(f"side_by_side: {e}", file=sys.stderr)
        sys.exit(1)
    print(f"side_by_side: {args.src} -> {args.dst}")


if __name__ == "__main__":
    main()
