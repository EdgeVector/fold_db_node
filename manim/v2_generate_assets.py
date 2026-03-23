"""
FoldDB V2 Brutalist Video — Asset Generator

Generates 6 background images (DALL-E 3) and 6 voiceover clips (OpenAI TTS)
for the brutalist explainer video.

Usage:
    export OPENAI_API_KEY="sk-..."
    python v2_generate_assets.py

Output: v2_assets/{scene_01..06}_bg.png and v2_assets/{scene_01..06}_vo.mp3
"""

import os
import sys
from pathlib import Path

from openai import OpenAI

ASSETS_DIR = Path(__file__).parent / "v2_assets"

SCENES = [
    {
        "id": "scene_01",
        "vo": "This is FoldDB.",
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene empty tundra landscape, "
            "pale gray sky, single dark monolith on flat ground, minimal and still. "
            "No text, no words, no letters."
        ),
    },
    {
        "id": "scene_02",
        "vo": (
            "In FoldDB, the smallest unit of data is called an atom. "
            "It's immutable — once created, it never changes. "
            "Store the same data twice? You get the same atom. No duplicates. "
            "You can't edit an atom. You can only create a new one."
        ),
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene Icelandic moss field, "
            "soft green ground stretching to distant mountains, overcast sky, "
            "quiet and vast. No text, no words, no letters."
        ),
    },
    {
        "id": "scene_03",
        "vo": (
            "So if atoms never change, how do you update anything? With a molecule. "
            "A molecule is a pointer — it points to an atom. When you update, "
            "FoldDB creates a new atom and moves the pointer. The old atom stays. "
            "That's version history, for free."
        ),
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene Norwegian fjord at dawn, "
            "still water reflecting pale sky, snow-dusted cliffs on both sides, "
            "calm and desolate. No text, no words, no letters."
        ),
    },
    {
        "id": "scene_04",
        "vo": (
            "You don't think in atoms and molecules. You think in fields — "
            "name, email, age. That's what a schema gives you. A schema is just "
            "a view — it maps field names to molecules. Two schemas can share "
            "the same molecule. Same data, two views. Nothing was copied."
        ),
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene volcanic black sand beach, "
            "layered horizon with dark ground, pale ocean, and soft blue sky, "
            "Iceland vibes, empty and peaceful. No text, no words, no letters."
        ),
    },
    {
        "id": "scene_05",
        "vo": (
            "Every field controls its own access. Not the table — the field. "
            "Different viewers see different data. Same atoms, different disclosure."
        ),
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene Arctic stone cairns on a "
            "barren plateau, muted colors, distant low sun near the horizon, "
            "vast empty sky. No text, no words, no letters."
        ),
    },
    {
        "id": "scene_06",
        "vo": (
            "Atoms are immutable. Molecules are pointers with version history. "
            "Schemas are flexible views. No migrations. Permissions protect every "
            "field. Semantic storage. Controlled disclosure. Data sovereignty."
        ),
        "bg_prompt": (
            "Pixel art, 16-bit retro style. Serene Northern Lights over a "
            "still frozen lake, faint aurora bands in green and blue, snow-covered "
            "flat landscape, silent and majestic. No text, no words, no letters."
        ),
    },
]


def generate_image(client: OpenAI, scene: dict) -> None:
    out_path = ASSETS_DIR / f"{scene['id']}_bg.png"
    if out_path.exists():
        print(f"  [skip] {out_path.name} already exists")
        return

    print(f"  [dall-e] Generating {out_path.name}...")
    response = client.images.generate(
        model="dall-e-3",
        prompt=scene["bg_prompt"],
        size="1792x1024",
        quality="hd",
        n=1,
    )
    image_url = response.data[0].url

    import httpx

    img_data = httpx.get(image_url).content
    out_path.write_bytes(img_data)
    print(f"  [done] {out_path.name} ({len(img_data)} bytes)")


def generate_voiceover(client: OpenAI, scene: dict) -> None:
    out_path = ASSETS_DIR / f"{scene['id']}_vo.mp3"
    if out_path.exists():
        print(f"  [skip] {out_path.name} already exists")
        return

    print(f"  [tts] Generating {out_path.name}...")
    response = client.audio.speech.create(
        model="tts-1-hd",
        voice="onyx",
        input=scene["vo"],
        response_format="mp3",
    )
    response.stream_to_file(str(out_path))
    print(f"  [done] {out_path.name}")


def main():
    if not os.environ.get("OPENAI_API_KEY"):
        print("Error: OPENAI_API_KEY not set")
        sys.exit(1)

    ASSETS_DIR.mkdir(parents=True, exist_ok=True)
    client = OpenAI()

    print("=== Generating background images ===")
    for scene in SCENES:
        generate_image(client, scene)

    print("\n=== Generating voiceover audio ===")
    for scene in SCENES:
        generate_voiceover(client, scene)

    print("\n=== Done ===")
    print(f"Assets in: {ASSETS_DIR}")
    for f in sorted(ASSETS_DIR.iterdir()):
        print(f"  {f.name}")


if __name__ == "__main__":
    main()
