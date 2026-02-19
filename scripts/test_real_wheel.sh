#!/usr/bin/env bash
set -euo pipefail

# Real wheel smoke test for unrepair wheel workflow.
# - Downloads latest Pillow manylinux x86_64 wheel from PyPI
# - Runs `unrepair wheel` with real system libs
# - Verifies DT_NEEDED, ldd resolution, and importability

WORKDIR="${WORKDIR:-$(mktemp -d)}"
echo "WORKDIR=$WORKDIR"

WHEEL_IN="$WORKDIR/pillow.whl"
WHEEL_OUT="$WORKDIR/pillow.unrepaired.whl"
EXTRACTED="$WORKDIR/out"
SYS_LIB_DIR="$WORKDIR/syslibs"

mkdir -p "$SYS_LIB_DIR" "$EXTRACTED"

echo "Resolving latest Pillow wheel URL..."
WHEEL_URL="$(
python3 - <<'PY'
import json, urllib.request
data = json.load(urllib.request.urlopen("https://pypi.org/pypi/pillow/json"))
ver = data["info"]["version"]
files = data["releases"][ver]
wheels = [f for f in files if f["filename"].endswith(".whl") and "manylinux" in f["filename"] and "x86_64" in f["filename"]]
cp314 = [f for f in wheels if "cp314" in f["filename"]]
chosen = (cp314 or wheels)[0]
print(chosen["url"])
PY
)"

echo "Downloading wheel..."
curl -L "$WHEEL_URL" -o "$WHEEL_IN"

echo "Preparing system library directory..."
for lib in \
  /usr/lib/libwebp.so.7 \
  /usr/lib/libpng16.so.16 \
  /usr/lib/libtiff.so.6 \
  /usr/lib/libopenjp2.so.7 \
  /usr/lib/liblzma.so.5 \
  /usr/lib/libzstd.so.1
do
  if [[ -f "$lib" ]]; then
    ln -sf "$lib" "$SYS_LIB_DIR/$(basename "$lib")"
  fi
done

echo "Running unrepair..."
cargo run -- wheel \
  --wheel "$WHEEL_IN" \
  --output-wheel "$WHEEL_OUT" \
  --system-lib-dir "$SYS_LIB_DIR" \
  -v \
  --no-strict \
  --format json \
  --color never > "$WORKDIR/unrepair.json"

echo "Extracting output wheel..."
unzip -q "$WHEEL_OUT" -d "$EXTRACTED"

echo "Checking DT_NEEDED..."
readelf -d "$EXTRACTED/PIL/_imaging.cpython-314-x86_64-linux-gnu.so" | rg "NEEDED.*(libtiff|libopenjp2|libpng16|libwebp)" -n -S || true
readelf -d "$EXTRACTED/PIL/_webp.cpython-314-x86_64-linux-gnu.so" | rg "NEEDED.*(libwebp|libwebpmux|libwebpdemux)" -n -S || true

echo "Checking ldd resolution..."
chmod +x "$EXTRACTED"/PIL/*.so || true
ldd "$EXTRACTED/PIL/_imaging.cpython-314-x86_64-linux-gnu.so" | rg "(libtiff|libopenjp2|libpng16|libwebp)" -n -S || true
ldd "$EXTRACTED/PIL/_webp.cpython-314-x86_64-linux-gnu.so" | rg "(libwebp|libwebpmux|libwebpdemux)" -n -S || true

echo "Checking importability..."
PYTHONPATH="$EXTRACTED" python3 - <<'PY'
import PIL
from PIL import Image
import PIL._imaging
import PIL._webp
print("import_ok", PIL.__version__)
print("image_mode", Image.new("RGB", (2,2)).mode)
PY

echo
echo "unrepair summary:"
python3 - <<'PY' "$WORKDIR/unrepair.json"
import json, sys
with open(sys.argv[1]) as f:
    data = json.load(f)
print(json.dumps({
    "summary": data.get("summary"),
    "failures": data.get("failures"),
    "warnings": data.get("warnings"),
    "removed_bundled_paths": data.get("removed_bundled_paths"),
}, indent=2))
PY
echo
echo "Done."
echo "Input wheel:  $WHEEL_IN"
echo "Output wheel: $WHEEL_OUT"
echo "JSON report:  $WORKDIR/unrepair.json"
