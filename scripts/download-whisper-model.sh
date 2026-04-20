#!/usr/bin/env bash
# Download the ggml Whisper model used by the audio connector.
# Default: base.en (~141MB, English, fast on Apple Silicon).
# Override with MODEL=medium.en ./download-whisper-model.sh for better quality
# (~1.5GB, slower). Skips the download if the target file already exists.

set -euo pipefail
source "$(dirname "$0")/lib.sh"

MODEL="${MODEL:-base.en}"
[[ "$MODEL" =~ ^[a-z0-9._-]+$ ]] || { echo "invalid MODEL (allowed chars: a-z 0-9 . _ -): $MODEL" >&2; exit 1; }
FILE="ggml-$MODEL.bin"
URL="https://huggingface.co/ggerganov/whisper.cpp/resolve/main/$FILE"
DEST="$ALVUM_MODELS_DIR/$FILE"
trap 'rm -f "$DEST.tmp"' EXIT

ensure_dirs

if [[ -f "$DEST" ]]; then
  echo "    $FILE already present ($(du -h "$DEST" | cut -f1))"
  echo "$DEST"
  exit 0
fi

echo "--> downloading $FILE from Hugging Face (one-time, progress below)"
curl -fL --progress-bar -o "$DEST.tmp" "$URL"
mv "$DEST.tmp" "$DEST"
echo "    $DEST"
