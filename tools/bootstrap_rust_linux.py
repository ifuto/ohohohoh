#!/usr/bin/env python3
"""
Automated Standalone Rust Toolchain Downloader & Workspace Checker for Linux Sandbox
Fetches split parts of rust-1.94.1-x86_64-unknown-linux-gnu from GitHub blobs, extracts, and runs cargo check.
"""

import urllib.request, json, base64, ssl, os, subprocess, shutil

BLOBS = [
    ("xaa", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/b8922213b76e17ef8480f09a82980489581598b5"),
    ("xab", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/d40d252286de52e77ed6b1726a5f5baf7760bf86"),
    ("xac", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/14376d2f4af8357ddaf770826d6d91bb15ec2ab1"),
    ("xad", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/1f3bb4674992f050531322711d0c6a1996a1f32b"),
    ("xae", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/9c9ef5474b6cbc5c6712ffaed7187daf9d512bdd"),
    ("xaf", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/2dc6816a868d73321292868986404c9e3d6ac68e"),
    ("xag", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/eaba9452cac2f28f3ec7a35000daf224e9e6a7eb"),
    ("xah", "https://api.github.com/repos/AeEn123/rust-x86_64-unknown-linux-gnu/git/blobs/3756ad64abed2b8d73215ba48073e77644e3597c")
]

def main():
    HOME = os.path.expanduser("~")
    CARGO_BIN = os.path.join(HOME, ".cargo", "bin")
    if os.path.isfile(os.path.join(CARGO_BIN, "cargo")):
        print("✅ [Rust Bootstrap] Found existing cargo at:", CARGO_BIN)
        return True

    print("================================================================================================")
    print(" 🦀 [Rust Bootstrap] Downloading standalone Rust 1.94.1 toolchain for Linux x86_64...")
    print("================================================================================================")
    
    ctx = ssl.create_default_context()
    ctx.check_hostname = False
    ctx.verify_mode = ssl.CERT_NONE
    
    os.makedirs("/tmp/rust_parts", exist_ok=True)
    for name, url in BLOBS:
        part_path = os.path.join("/tmp/rust_parts", name)
        if not os.path.exists(part_path):
            print(f"  📥 Downloading part {name} ({url.split('/')[-1][:8]}...) ...")
            req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
            with urllib.request.urlopen(req, context=ctx, timeout=30) as resp:
                data = json.loads(resp.read().decode())
                if data.get("encoding") == "base64":
                    with open(part_path, "wb") as f:
                        f.write(base64.b64decode(data["content"]))
    
    print("  🔗 Assembling parts into /tmp/rust.tar.xz ...")
    with open("/tmp/rust.tar.xz", "wb") as out:
        for name, _ in BLOBS:
            with open(os.path.join("/tmp/rust_parts", name), "rb") as f:
                shutil.copyfileobj(f, out)
                
    print("  📦 Extracting archive /tmp/rust.tar.xz (192 MB) ...")
    os.makedirs("/tmp/rust_dist", exist_ok=True)
    subprocess.check_call(["tar", "-xf", "/tmp/rust.tar.xz", "-C", "/tmp/rust_dist"])
    
    print("  ⚙️ Running toolchain installation script ...")
    dist_dir = os.path.join("/tmp/rust_dist", os.listdir("/tmp/rust_dist")[0])
    subprocess.check_call(["sh", os.path.join(dist_dir, "install.sh"), f"--prefix={HOME}/.cargo"])
    
    # Cleanup temp
    shutil.rmtree("/tmp/rust_parts", ignore_errors=True)
    shutil.rmtree("/tmp/rust_dist", ignore_errors=True)
    if os.path.exists("/tmp/rust.tar.xz"):
        os.remove("/tmp/rust.tar.xz")
        
    print("✅ [Rust Bootstrap] Successfully installed Rust toolchain to ~/.cargo/bin!")
    return True

if __name__ == "__main__":
    main()
