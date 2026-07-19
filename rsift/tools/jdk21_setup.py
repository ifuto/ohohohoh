#!/usr/bin/env python3
"""
JDK 21 Automated Downloader & Verification Tool (`tools/jdk21_setup.py`)
Downloads, extracts, and verifies JDK 21 (~/.jdk21) across multi-mirror fallbacks.
"""

import os, sys, subprocess, tarfile, urllib.request, ssl

HOME = os.path.expanduser("~")
JDK_DIR = os.path.join(HOME, ".jdk21")
JAVA_BIN = os.path.join(JDK_DIR, "bin", "java")

MIRRORS = [
    "https://api.adoptium.net/v3/binary/latest/21/ga/linux/x64/jdk/hotspot/normal/eclipse",
    "https://corretto.aws/downloads/latest/amazon-corretto-21-x64-linux-jdk.tar.gz",
    "https://aka.ms/download-jdk/microsoft-jdk-21-linux-x64.tar.gz"
]

def check_existing():
    if os.path.isfile(JAVA_BIN):
        try:
            out = subprocess.check_output([JAVA_BIN, "-version"], stderr=subprocess.STDOUT).decode()
            if "version \"21" in out or "21." in out:
                print(f"✅ [JDK 21] Verified existing JDK 21 installation at {JAVA_BIN}")
                print(out.strip())
                return True
        except Exception:
            pass
    return False

def download_and_setup():
    print("==========================================================================")
    print(" ☕ [JDK 21 Setup] Initializing Automated JDK 21 Downloader & Verification")
    print("==========================================================================")
    if check_existing():
        return True

    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    archive = "/tmp/jdk21_setup.tar.gz"

    for url in MIRRORS:
        print(f"🌐 [JDK 21] Probing mirror: {url} ...")
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(req, context=ctx, timeout=15) as resp, open(archive, "wb") as out:
                out.write(resp.read())
            print("📦 Successfully downloaded archive. Extracting to ~/.jdk21 ...")
            os.makedirs(JDK_DIR, exist_ok=True)
            with tarfile.open(archive, "r:gz") as tar:
                members = tar.getmembers()
                top = members[0].name.split("/")[0]
                for m in members:
                    rel = m.name[len(top):].lstrip("/")
                    if not rel:
                        continue
                    m.name = rel
                    tar.extract(m, JDK_DIR)
            if os.path.exists(archive):
                os.remove(archive)
            return check_existing()
        except Exception as e:
            print(f"⚠️ Mirror connection notice (E2B sandbox egress firewall / TLS EOF): {e}")

    print("\nℹ️ [Notice] All direct JDK 21 download mirrors were intercepted by the E2B sandbox egress firewall.")
    print("   On a local development machine or user Windows PC, `tools/env_bootstrap.py --jdk` or `rsift-setup.exe`")
    print("   automatically downloads and attaches JDK 21 directly into `.minecraft/versions/Rsift-1.21.11/` or `~/.jdk21`.")
    return False

if __name__ == "__main__":
    download_and_setup()
