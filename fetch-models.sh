#!/bin/sh
# Downloads the ocrs OCR models that get embedded into the binary via
# include_bytes!. Must be run once before `cargo build`.
# Models: https://github.com/robertknight/ocrs-models (Apache-2.0 / MIT)
set -eu

dir="$(dirname "$0")/models"
mkdir -p "$dir"

base="https://ocrs-models.s3-accelerate.amazonaws.com"
for f in text-detection.rten text-recognition.rten; do
    if [ -s "$dir/$f" ]; then
        echo "$f already present, skipping"
    else
        echo "downloading $f ..."
        curl -fL --progress-bar -o "$dir/$f" "$base/$f"
    fi
done
echo "done. models in $dir"
