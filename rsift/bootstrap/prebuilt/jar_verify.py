import sys, zipfile
jar = sys.argv[1]
try:
    z = zipfile.ZipFile(jar)
    data = z.read("com/rsift/RsiftHooks.class")
    if b"resolveField" in data and b"nativeResolveField0" in data:
        print("OK")
        sys.exit(0)
    else:
        print("STALE: resolveField not found in RsiftHooks.class")
        sys.exit(1)
except Exception as e:
    print(f"ERROR: {e}")
    sys.exit(1)
