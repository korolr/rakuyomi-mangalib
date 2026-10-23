#!/usr/bin/env python3
"""Live check of the bundled lib.social sources with a real account.

Starts the debug server with a throw-away home, imports the tokens into the
MangaLib source (linked sources get them from the host), then for every case
searches, loads details/chapters and downloads the first unlocked chapter.

Tokens are NEVER printed: they come from MANGALIB_ACCESS_TOKEN /
MANGALIB_REFRESH_TOKEN or from --secrets FILE ({"access_token", "refresh_token"},
mode 600, outside the repository), and every output line and the server log are
checked/redacted.

  fork/build.sh dev
  python3 fork/e2e/live_tokens.py --secrets ~/.secrets/mangalib_tokens.json
  python3 fork/e2e/live_tokens.py --secrets FILE --force-refresh   # test the refresh path

--force-refresh imports an expired access token, so the source must refresh:
the refresh token ROTATES (the old pair is revoked), the new pair is written
back to --secrets (required with this flag).

Cases: --case SOURCE_ID:QUERY[:EXPECTED_MANGA_ID] (repeatable);
default ru.mangalib.tokens:Eleceed:7072--eleceed- (an age-restricted title: it
is only found with a valid token).
"""
import argparse, base64, json, os, shutil, socket, subprocess, sys, tempfile, threading, time
import urllib.error, urllib.parse, urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SECRETS = []


def red(s):
    s = str(s)
    for v in SECRETS:
        s = s.replace(v, "<REDACTED>")
    return s


def out(*a):
    print(red(" ".join(str(x) for x in a)), flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def walk_ids(obj):
    """Yields every {"source_id", "manga_id"} object of a (nested) response."""
    if isinstance(obj, dict):
        if "source_id" in obj and "manga_id" in obj:
            yield obj
        elif isinstance(obj.get("id"), str) and isinstance(obj.get("source"), dict):
            yield {"source_id": obj["source"].get("id"), "manga_id": obj["id"]}
        else:
            for v in obj.values():
                yield from walk_ids(v)
    elif isinstance(obj, list):
        for v in obj:
            yield from walk_ids(v)


def rotated_pair(home, refresh):
    """Returns (access, refresh, client) stored by any token source if it differs from `refresh`."""
    try:
        st = json.load(open(home + "/settings.json")).get("source_settings", {})
    except Exception:
        return None
    for sid, v in st.items():
        rt = v.get("refreshToken") if isinstance(v, dict) else None
        if rt and refresh and rt != refresh and v.get("accessToken"):
            return v["accessToken"], rt, v.get("authClientId")
    return None


def save_pair(path, access, refresh, client_id):
    tmp = path + ".tmp"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        json.dump({"access_token": access, "refresh_token": refresh, "client_id": client_id,
                   "saved_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}, f)
    os.replace(tmp, path)
    os.chmod(path, 0o600)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--secrets", help="token file (600) outside the repo; updated after a rotation")
    ap.add_argument("--force-refresh", action="store_true")
    ap.add_argument("--anonymous", action="store_true", help="do not import any token")
    ap.add_argument("--case", action="append", default=[])
    ap.add_argument("--server", default=os.path.join(ROOT, "backend/target/debug/server"))
    ap.add_argument("--sources", default=os.path.join(ROOT, "build/rakuyomi-mangalib.koplugin/bundled-sources"))
    ap.add_argument("--keep-home", action="store_true")
    args = ap.parse_args()

    if args.secrets:
        d = json.load(open(args.secrets))
        access, refresh = d.get("access_token"), d.get("refresh_token")
    else:
        access, refresh = os.environ.get("MANGALIB_ACCESS_TOKEN"), os.environ.get("MANGALIB_REFRESH_TOKEN")
    SECRETS.extend(v for v in (access, refresh) if v)
    if args.force_refresh and not args.secrets:
        sys.exit("--force-refresh rotates the pair: pass --secrets FILE so the new pair is saved")
    if not args.anonymous and not refresh:
        sys.exit("no tokens: set MANGALIB_ACCESS_TOKEN/MANGALIB_REFRESH_TOKEN or pass --secrets")
    cases = [c.split(":") for c in (args.case or ["ru.mangalib.tokens:Eleceed:7072--eleceed-"])]

    home = tempfile.mkdtemp(prefix="rakuyomi-live-")
    os.chmod(home, 0o700)
    os.makedirs(home + "/sources"); os.makedirs(home + "/import")
    for f in os.listdir(args.sources):
        shutil.copy(os.path.join(args.sources, f), home + "/sources/")
    imported = access
    if not args.anonymous:
        if args.force_refresh:
            b = lambda o: base64.urlsafe_b64encode(json.dumps(o).encode()).rstrip(b"=").decode()
            aud = "1"
            try:
                aud = str(json.loads(base64.urlsafe_b64decode(access.split(".")[1] + "==")).get("aud") or "1")
            except Exception:
                pass
            imported = b({"typ": "JWT", "alg": "RS256"}) + "." + b({"aud": aud, "exp": int(time.time()) - 3600, "sub": "0"}) + ".c2ln"
        fd = os.open(home + "/import/ru.mangalib.tokens.json", os.O_WRONLY | os.O_CREAT, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump({"accessToken": imported, "refreshToken": refresh}, f)

    port = free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("MANGALIB_")}
    env.update(RAKUYOMI_TCP_PORT=str(port), RUST_LOG="info", RAKUYOMI_NET_DEBUG="1")
    log_path = home + "/server.log"
    log = open(log_path, "wb")
    proc = subprocess.Popen([args.server, home], stdout=log, stderr=subprocess.STDOUT, env=env)

    # Watchdog: the moment a rotated pair lands in settings.json, copy it to
    # --secrets, so the new (only valid) pair survives any crash below.
    saved = {"pair": None}
    stop = threading.Event()

    def watch():
        while not stop.is_set():
            rot = rotated_pair(home, refresh)
            if rot and args.secrets and rot[:2] != saved["pair"]:
                save_pair(args.secrets, *rot)
                saved["pair"] = rot[:2]
            stop.wait(0.2)

    watcher = threading.Thread(target=watch, daemon=True)
    watcher.start()

    def call(method, path, body=None, timeout=180):
        data = None if body is None and method == "GET" else json.dumps(body).encode()
        req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", data=data, method=method,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=timeout) as r:
                return json.loads(r.read() or b"null")
        except urllib.error.HTTPError as e:
            raise RuntimeError(f"{method} {path}: HTTP {e.code}: {red(e.read()[:300])}")

    q = lambda s: urllib.parse.quote(s, safe="")
    ok = True
    try:
        for _ in range(200):
            try:
                call("GET", "/installed-sources"); break
            except Exception:
                time.sleep(0.3)
        for case in cases:
            source, query = case[0], case[1]
            expect = case[2] if len(case) > 2 and case[2] else None
            try:
                res = call("GET", f"/mangas?q={q(query)}")
                ids = [m["manga_id"] for m in walk_ids(res) if m.get("source_id") == source]
                out(f"[{source}] search {query!r}: {len(ids)} results")
                manga = expect or ids[0]
                assert manga in ids, f"{manga} not found (token not applied?)"
                call("POST", f"/mangas/{q(source)}/{q(manga)}/refresh-details", None)
                call("POST", f"/mangas/{q(source)}/{q(manga)}/refresh-chapters", None)
                chs = call("GET", f"/mangas/{q(source)}/{q(manga)}/chapters")
                chs = chs[0] if chs and isinstance(chs[0], list) else chs
                free = [c for c in chs if not c.get("locked")]
                out(f"[{source}] {manga}: {len(chs)} chapters, {len(chs) - len(free)} locked")
                ch = sorted(free, key=lambda c: (float(c.get("volume_num") or 0), float(c.get("chapter_num") or 0)))[0]
                job = call("POST", "/jobs/download-chapter", {"source_id": source, "manga_id": manga, "chapter_id": ch["id"]["chapter_id"] if isinstance(ch["id"], dict) else ch["id"]})
                for _ in range(300):
                    d = call("GET", f"/jobs/{job}")
                    if d.get("type") != "PENDING":
                        break
                    time.sleep(1)
                out(f"[{source}] download vol {ch.get('volume_num')} ch {ch.get('chapter_num')}: {d.get('type')}")
                ok &= d.get("type") == "COMPLETED"
            except Exception as e:
                ok = False
                out(f"[{source}] FAILED: {e}")
    finally:
        # Persisting settings is asynchronous: give a rotation time to reach
        # settings.json before stopping the server, or the new pair is lost
        # (the old one is already revoked).
        rot = None
        for _ in range(30):
            rot = rotated_pair(home, refresh)
            if rot or "oauth/token -> 200" not in open(log_path, "rb").read().decode(errors="replace"):
                break
            time.sleep(1)
        proc.terminate(); proc.wait(10); log.close()
        stop.set(); watcher.join()

    rot = rotated_pair(home, refresh)
    if not rot and "oauth/token -> 200" in open(log_path, "rb").read().decode(errors="replace"):
        ok = False
        args.keep_home = True
        out("WARNING: a refresh succeeded but the new pair is not in settings.json; home kept:", home)
    if rot:
        new_at, new_rt, client = rot
        SECRETS.extend([new_at, new_rt])
        out("token pair rotated by the source (old pair is revoked now)")
        if args.secrets:
            if saved["pair"] != (new_at, new_rt):
                save_pair(args.secrets, new_at, new_rt, client)
            out("new pair saved to", args.secrets)
        else:
            out("WARNING: the new pair is only in", home + "/settings.json (kept)")
            args.keep_home = True
    elif args.force_refresh:
        ok = False
        out("refresh did NOT happen")
    text = open(log_path, "rb").read().decode(errors="replace")
    leaks = sum(text.count(v) for v in SECRETS if v)
    for line in text.splitlines():
        if "[mangalib]" in line or "oauth/token" in line:
            out("log:", line[:200])
    out("token values in server log:", leaks)
    if not args.keep_home and ok:
        shutil.rmtree(home, ignore_errors=True)
    if os.path.isdir(home):
        out("home kept (mode 700, may contain tokens; delete it when done):", home)
    out("RESULT:", "OK" if ok and not leaks else "FAIL")
    sys.exit(0 if ok and not leaks else 1)


main()
