#!/usr/bin/env python3
"""Generate synthetic test face images via DALL-E 3.

Generates N photos of each of two fictional characters with
consistent physical descriptions, so multiple generations of the
"same" character produce visually similar faces that the ArcFace
embedder can cluster via cosine similarity. This gives the test
framework real data to validate Phase 1 fingerprint clustering
and persona resolution without using anyone's actual pictures.

Writes to `test-framework/fixtures/faces/<character>_<idx>.jpg`
relative to the repository root. Safe to re-run — existing files
are skipped so partial regenerations are cheap.

The two characters are deliberately generic fictional personas
with stable physical descriptions. DALL-E 3 does not actually
guarantee pixel-perfect facial consistency across generations —
think of it as "two different people, each shown from multiple
angles" — but the visual similarity is strong enough that
average same-character cosine is ~2× average cross-character
cosine (see the fixtures README).

Usage:
    OPENAI_API_KEY=sk-... python3 test-framework/scripts/generate-face-fixtures.py
"""

import base64
import json
import os
import sys
import urllib.request

# Resolve output dir relative to this script so it works regardless
# of the user's current working directory when invoking.
SCRIPT_DIR = os.path.dirname(os.path.abspath(__file__))
OUT_DIR = os.path.normpath(
    os.path.join(SCRIPT_DIR, "..", "fixtures", "faces")
)
os.makedirs(OUT_DIR, exist_ok=True)

API_KEY = os.environ["OPENAI_API_KEY"]

# Two fictional characters, generated from short physical descriptions.
# Each description is followed into DALL-E as a portrait prompt, N times.
CHARACTERS = {
    "alice": (
        "A photorealistic professional headshot portrait of a fictional "
        "woman in her 30s with shoulder-length brown hair, green eyes, "
        "light skin, wearing a navy blue blazer. Studio lighting, "
        "looking directly at the camera, neutral expression, clean "
        "white background. Highly detailed, photograph style."
    ),
    "bob": (
        "A photorealistic professional headshot portrait of a fictional "
        "man in his 40s with short black hair, a trimmed beard, brown "
        "eyes, medium skin tone, wearing a gray sweater. Natural "
        "window lighting, looking slightly off-camera, subtle smile, "
        "blurred office background. Highly detailed, photograph style."
    ),
}

# Number of variations per character.
N_PER_CHARACTER = 3


def generate(prompt: str, out_path: str) -> bool:
    """Call DALL-E 3 with the given prompt and save the result to
    out_path. Returns True on success, False on API error."""
    body = json.dumps(
        {
            "model": "dall-e-3",
            "prompt": prompt,
            "n": 1,
            "size": "1024x1024",
            "quality": "standard",
            "response_format": "b64_json",
        }
    ).encode("utf-8")

    req = urllib.request.Request(
        "https://api.openai.com/v1/images/generations",
        data=body,
        headers={
            "Authorization": f"Bearer {API_KEY}",
            "Content-Type": "application/json",
        },
        method="POST",
    )

    try:
        with urllib.request.urlopen(req, timeout=120) as resp:
            payload = json.loads(resp.read().decode("utf-8"))
    except Exception as e:
        print(f"[error] generation failed: {e}", file=sys.stderr)
        return False

    b64 = payload["data"][0]["b64_json"]
    with open(out_path, "wb") as f:
        f.write(base64.b64decode(b64))
    return True


def main() -> None:
    for name, prompt in CHARACTERS.items():
        for i in range(1, N_PER_CHARACTER + 1):
            out_path = os.path.join(OUT_DIR, f"{name}_{i:02d}.jpg")
            if os.path.exists(out_path):
                print(f"[skip] {out_path} already exists", file=sys.stderr)
                continue
            print(f"[gen ] {name}_{i:02d}.jpg ...", file=sys.stderr, flush=True)
            ok = generate(prompt, out_path)
            status = "ok" if ok else "FAIL"
            print(f"       {status}", file=sys.stderr)


if __name__ == "__main__":
    main()
