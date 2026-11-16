"""A local Mattermost to try debug builds against, so testing never touches a real account.

    python dev/test-server.py up      start it, add the test accounts and channels, log the debug build in
    python dev/test-server.py down    stop it (it keeps its data)
    python dev/test-server.py reset   delete it; the next `up` starts from nothing

Runs mattermost/mattermost-preview in Docker, reachable only from this PC. `up` can be
run again at any time: what already exists is kept.

Debug builds (`cargo run`) use their own profile, `mmsend-dev`, which `up` logs in as
`tester`. Release builds never see any of this.
"""

import ctypes
import ctypes.wintypes as wt
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

CONTAINER = "mmsend-test-mattermost"
IMAGE = "mattermost/mattermost-preview:latest"
PORT = 8065
# Not `localhost`: that resolves to ::1 first, which the IPv4-only port mapping leaves
# hanging until a timeout, on every request.
SERVER = f"http://127.0.0.1:{PORT}"
READY_TIMEOUT_SECONDS = 180
# Throwaway accounts on a server only this PC can reach.
PASSWORD = "mmsend-test-1"
# The debug build's profile: its settings folder and Credential Manager entry (src/store.rs).
DEV_PROFILE = "mmsend-dev"

ME = "tester"
# username: (last name, first name). Chinese names, as the real directory has.
PEOPLE = {
    ME: ("測", "試員"),
    "alice": ("王", "小明"),
    "bob": ("李", "大華"),
    "carol": ("陳", "美玲"),
    "dave": ("林", "志豪"),
}
TEAM = ("dev", "開發部")
# name: (display name, type, whether tester is a member). One public channel tester has
# not joined: it can be found, yet refuses a post, which a dry run has to catch.
CHANNELS = {
    "announcements": ("公告", "O", True),
    "project": ("專案小組", "P", True),
    "elsewhere": ("其他部門", "O", False),
}


def docker(*args, check=True):
    return subprocess.run(
        ["docker", *args], capture_output=True, text=True, encoding="utf-8", errors="replace", check=check
    )


def api(method, path, body=None, token=None):
    headers = {"Content-Type": "application/json"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    data = None if body is None else json.dumps(body).encode("utf-8")
    request = urllib.request.Request(f"{SERVER}/api/v4{path}", data=data, method=method, headers=headers)
    with urllib.request.urlopen(request, timeout=20) as response:
        text = response.read().decode("utf-8")
        return json.loads(text) if text else None, response.headers


def start():
    names = docker("ps", "-a", "--filter", f"name=^{CONTAINER}$", "--format", "{{.Names}}").stdout.split()
    if CONTAINER in names:
        docker("start", CONTAINER)
    else:
        print(f"建立容器 {CONTAINER}…")
        docker("run", "--name", CONTAINER, "-d", "-p", f"127.0.0.1:{PORT}:{PORT}", IMAGE)
    print("等待伺服器啟動…", end="", flush=True)
    deadline = time.time() + READY_TIMEOUT_SECONDS
    while time.time() < deadline:
        try:
            status, _ = api("GET", "/system/ping")
            if status.get("status") == "OK":
                print(" 好了")
                return
        except (urllib.error.URLError, ConnectionError, OSError):
            pass
        print(".", end="", flush=True)
        time.sleep(3)
    raise SystemExit(f"\n{READY_TIMEOUT_SECONDS} 秒內沒有啟動，請看 docker logs {CONTAINER}")


def login(username):
    user, headers = api("POST", "/users/login", {"login_id": username, "password": PASSWORD})
    return user, headers["Token"]


def ensure_user(username, admin_token=None):
    last, first = PEOPLE[username]
    try:
        return login(username)[0]
    except urllib.error.HTTPError:
        pass
    # The first account on a new server becomes its system admin, so tester goes first.
    user, _ = api(
        "POST",
        "/users",
        {"username": username, "email": f"{username}@example.com", "password": PASSWORD,
         "first_name": first, "last_name": last},
        admin_token,
    )
    print(f"  建立帳號 {username}（{last}{first}）")
    return user


def ensure_team(token):
    name, display_name = TEAM
    try:
        team, _ = api("GET", f"/teams/name/{name}", token=token)
    except urllib.error.HTTPError:
        team, _ = api("POST", "/teams", {"name": name, "display_name": display_name, "type": "O"}, token)
        print(f"  建立團隊 {display_name}")
    return team


def ensure_channel(token, team_id, name, display_name, kind):
    try:
        channel, _ = api("GET", f"/teams/{team_id}/channels/name/{name}?include_deleted=false", token=token)
    except urllib.error.HTTPError:
        channel, _ = api(
            "POST", "/channels", {"team_id": team_id, "name": name, "display_name": display_name, "type": kind}, token
        )
        print(f"  建立頻道 {display_name}")
    return channel


def seed():
    me = ensure_user(ME)
    _, token = login(ME)
    users = {ME: me} | {name: ensure_user(name, token) for name in PEOPLE if name != ME}
    team = ensure_team(token)
    for user in users.values():
        api("POST", f"/teams/{team['id']}/members", {"team_id": team["id"], "user_id": user["id"]}, token)
    for name, (display_name, kind, joined) in CHANNELS.items():
        channel = ensure_channel(token, team["id"], name, display_name, kind)
        if not joined:
            try:
                api("DELETE", f"/channels/{channel['id']}/members/{me['id']}", token=token)
            except urllib.error.HTTPError:
                pass  # already not a member
    return me, token


class CREDENTIALW(ctypes.Structure):
    _fields_ = [("Flags", wt.DWORD), ("Type", wt.DWORD), ("TargetName", wt.LPWSTR),
                ("Comment", wt.LPWSTR), ("LastWritten", wt.FILETIME),
                ("CredentialBlobSize", wt.DWORD), ("CredentialBlob", ctypes.POINTER(ctypes.c_ubyte)),
                ("Persist", wt.DWORD), ("AttributeCount", wt.DWORD), ("Attributes", ctypes.c_void_p),
                ("TargetAlias", wt.LPWSTR), ("UserName", wt.LPWSTR)]


CRED_TYPE_GENERIC = 1
CRED_PERSIST_LOCAL_MACHINE = 2


def log_in_debug_build(me, token):
    """What logging in from the window saves (src/store.rs: Config and Secrets), without typing."""
    folder = os.path.join(os.environ["APPDATA"], DEV_PROFILE)
    os.makedirs(folder, exist_ok=True)
    config = {"server_url": f"{SERVER}/", "login_id": ME, "username": me["username"], "user_id": me["id"]}
    with open(os.path.join(folder, "config.json"), "w", encoding="utf-8") as f:
        json.dump(config, f, ensure_ascii=False, indent=2)
    blob = json.dumps({"password": PASSWORD, "token": token}).encode("utf-8")
    buffer = (ctypes.c_ubyte * len(blob)).from_buffer_copy(blob)
    credential = CREDENTIALW(
        Type=CRED_TYPE_GENERIC, TargetName=DEV_PROFILE, CredentialBlobSize=len(blob),
        CredentialBlob=ctypes.cast(buffer, ctypes.POINTER(ctypes.c_ubyte)),
        Persist=CRED_PERSIST_LOCAL_MACHINE, UserName=DEV_PROFILE,
    )
    if not ctypes.WinDLL("advapi32", use_last_error=True).CredWriteW(ctypes.byref(credential), 0):
        raise SystemExit(f"無法寫入認證管理員（錯誤碼 {ctypes.get_last_error()}）")


def main():
    # Line buffered: progress shows as it happens even when the output is piped.
    sys.stdout.reconfigure(encoding="utf-8", line_buffering=True)
    command = sys.argv[1] if len(sys.argv) > 1 else ""
    if command == "up":
        start()
        me, token = seed()
        log_in_debug_build(me, token)
        print(f"完成：{SERVER}（網頁也能開），開發版已登入為 {ME}，密碼 {PASSWORD}")
    elif command == "down":
        docker("stop", CONTAINER, check=False)
        print("已停止")
    elif command == "reset":
        docker("rm", "-f", CONTAINER, check=False)
        print("已刪除，下次 up 會重新建立")
    else:
        raise SystemExit(__doc__)


if __name__ == "__main__":
    main()
