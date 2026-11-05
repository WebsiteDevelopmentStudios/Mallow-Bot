#!/usr/bin/env python3
import base64, io, json, os, sys, tarfile, time, urllib.error, urllib.request
from pathlib import Path
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
ROOT=Path("/opt/mcxboxbroadcast")
REPO=os.environ.get("MCXB_BACKUP_REPO","WebsiteDevelopmentStudios/mcxboxbroadcast-backups")
PATH=os.environ.get("MCXB_BACKUP_PATH","state.tar.gz.enc")
TOKEN=os.environ.get("MCXB_BACKUP_TOKEN","")
KEY=os.environ.get("MCXB_BACKUP_ENCRYPTION_KEY","")
INTERVAL=max(120,int(os.environ.get("MCXB_BACKUP_INTERVAL","300")))
def key_bytes():
    value=base64.urlsafe_b64decode(KEY+"="*(-len(KEY)%4))
    if len(value)!=32: raise RuntimeError("MCXB_BACKUP_ENCRYPTION_KEY must decode to 32 bytes")
    return value
def api(method="GET",payload=None):
    url=f"https://api.github.com/repos/{REPO}/contents/{PATH}"
    data=json.dumps(payload).encode() if payload is not None else None
    req=urllib.request.Request(url,data=data,method=method,headers={"Authorization":f"Bearer {TOKEN}","Accept":"application/vnd.github+json","X-GitHub-Api-Version":"2022-11-28","User-Agent":"MCXboxBroadcast-Backup","Content-Type":"application/json"})
    try:
        with urllib.request.urlopen(req,timeout=30) as r: return json.loads(r.read())
    except urllib.error.HTTPError as e:
        detail=e.read().decode(errors="replace")
        if e.code==404 and method=="GET": return None
        raise RuntimeError(f"GitHub API HTTP {e.code}: {detail[:300]}")
def check_repository_access():
    url=f"https://api.github.com/repos/{REPO}"
    req=urllib.request.Request(url,headers={"Authorization":f"Bearer {TOKEN}","Accept":"application/vnd.github+json","X-GitHub-Api-Version":"2022-11-28","User-Agent":"MCXboxBroadcast-Backup"})
    try:
        with urllib.request.urlopen(req,timeout=30) as r:
            info=json.loads(r.read())
        print(f"GitHub backup repository access confirmed: {info.get('full_name',REPO)}",flush=True)
    except urllib.error.HTTPError as e:
        detail=e.read().decode(errors="replace")
        if e.code==404:
            raise RuntimeError(f"GitHub token cannot see repository {REPO}. Check that the fine-grained PAT is owned by the WebsiteDevelopmentStudios organization, has this exact repository selected, and has Contents: Read and write. GitHub hides inaccessible private repositories as 404. Details: {detail[:200]}")
        raise RuntimeError(f"GitHub repository access check failed with HTTP {e.code}: {detail[:250]}")

def archive():
    b=io.BytesIO()
    with tarfile.open(fileobj=b,mode="w:gz") as t:
        for n in ("config.yml","cache","screenshot.jpg"):
            p=ROOT/n
            if p.exists(): t.add(p,arcname=n)
    return b.getvalue()
def encrypt(data):
    nonce=os.urandom(12)
    return b"MCXB1"+nonce+AESGCM(key_bytes()).encrypt(nonce,data,b"mcxboxbroadcast-state-v1")
def decrypt(data):
    if not data.startswith(b"MCXB1") or len(data)<34: raise RuntimeError("Invalid encrypted backup")
    return AESGCM(key_bytes()).decrypt(data[5:17],data[17:],b"mcxboxbroadcast-state-v1")
def restore():
    item=api()
    if not item: print("No previous backup found; starting fresh.",flush=True); return
    data=decrypt(base64.b64decode(item.get("content","").replace("\n","")))
    ROOT.mkdir(parents=True,exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data),mode="r:gz") as t:
        members=t.getmembers()
        for m in members:
            p=Path(m.name)
            if p.is_absolute() or ".." in p.parts or not p.parts or p.parts[0] not in {"config.yml","cache","screenshot.jpg"}:
                raise RuntimeError("Unsafe path in backup archive")
        t.extractall(ROOT)
    print("Restored encrypted MCXboxBroadcast state.",flush=True)
def upload():
    payload={"message":"Update encrypted MCXboxBroadcast state","content":base64.b64encode(encrypt(archive())).decode("ascii")}
    old=api()
    if old and old.get("sha"): payload["sha"]=old["sha"]
    try: api("PUT",payload)
    except RuntimeError as e:
        if "HTTP 409" not in str(e) and "HTTP 422" not in str(e): raise
        latest=api()
        if latest and latest.get("sha"): payload["sha"]=latest["sha"]
        api("PUT",payload)
    print("Uploaded encrypted MCXboxBroadcast state.",flush=True)
def main():
    if not TOKEN or not KEY: print("Encrypted backup disabled: set MCXB_BACKUP_TOKEN and MCXB_BACKUP_ENCRYPTION_KEY.",flush=True); return
    key_bytes()
    if len(sys.argv)>1 and sys.argv[1]=="--restore": restore(); return
    while True:
        try:
            check_repository_access()
            upload()
        except Exception as e: print(f"Backup failed: {e}",flush=True)
        time.sleep(INTERVAL)
if __name__=="__main__": main()
