"""
FoldDB V2 Brutalist Video — Manim Composition

Loads DALL-E backgrounds + TTS voiceover from v2_assets/,
overlays animated foreground elements synced to audio.

Preview:  manim -pql v2_brutalist_video.py FoldDBBrutalist
Final:    manim -pqh v2_brutalist_video.py FoldDBBrutalist
"""

from pathlib import Path

from manim import *
from mutagen.mp3 import MP3

ASSETS = Path(__file__).parent / "v2_assets"

# Brutalist palette
GRAY_BG = "#D9D9D9"
BLACK = "#1A1A1A"
GREEN = "#2ECC40"
GREEN_DARK = "#1B7A28"
WHITE_BLOCK = "#F5F5F5"
BLUE = "#0074D9"
MAGENTA = "#FF3399"
RED = "#FF4136"
ALLOW_GREEN = "#2ECC40"
DENY_RED = "#FF4136"


def audio_duration(scene_id: str) -> float:
    """Get duration of a voiceover MP3 in seconds."""
    path = ASSETS / f"{scene_id}_vo.mp3"
    return MP3(str(path)).info.length


def load_bg(scene_id: str) -> ImageMobject:
    """Load a background image scaled to fill the frame."""
    path = ASSETS / f"{scene_id}_bg.png"
    img = ImageMobject(str(path))
    img.height = config.frame_height
    if img.width < config.frame_width:
        img.width = config.frame_width
    return img


def brutal_text(text: str, color=BLACK, font_size=48, weight=BOLD) -> Text:
    return Text(text, color=color, font_size=font_size, weight=weight, font="Courier New")


def brutal_rect(w, h, color=BLACK, fill_opacity=0.9) -> Rectangle:
    return Rectangle(
        width=w, height=h,
        color=color, fill_color=color, fill_opacity=fill_opacity,
        stroke_width=2, stroke_color=BLACK,
    )


class FoldDBBrutalist(Scene):
    def construct(self):
        self.camera.background_color = GRAY_BG
        self.scene_01_title()
        self.scene_02_atoms()
        self.scene_03_molecules()
        self.scene_04_schemas()
        self.scene_05_access()
        self.scene_06_full_picture()

    def safe_wait(self, seconds):
        """Wait only if duration is positive."""
        if seconds > 0.1:
            self.wait(seconds)

    # ── Scene 1: Title ───────────────────────────────────────────────

    def scene_01_title(self):
        dur = audio_duration("scene_01")
        bg = load_bg("scene_01")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_01_vo.mp3"))

        title_block = brutal_rect(5, 1.2, BLACK)
        title_text = brutal_text("FOLDDB", WHITE_BLOCK, font_size=72)
        title = VGroup(title_block, title_text)

        fade_in = min(0.8, dur * 0.5)
        self.play(FadeIn(title, scale=0.9), run_time=fade_in)
        self.safe_wait(dur - fade_in)
        self.play(FadeOut(title, bg), run_time=0.4)

    # ── Scene 2: Atoms ───────────────────────────────────────────────

    def scene_02_atoms(self):
        dur = audio_duration("scene_02")
        bg = load_bg("scene_02")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_02_vo.mp3"))

        # Budget: animations ~10s, waits fill the rest
        # Proportional: appear(20%), dedup(20%), reject(20%), new atom(15%), hold(25%)
        t_appear = dur * 0.20
        t_dedup = dur * 0.20
        t_reject = dur * 0.20
        t_new = dur * 0.15
        t_hold = dur * 0.20

        # Atom appears
        atom_block = brutal_rect(2.5, 0.8, GREEN)
        atom_label = brutal_text('"hello"', WHITE_BLOCK, font_size=36)
        atom = VGroup(atom_block, atom_label).move_to(ORIGIN)
        header = brutal_text("ATOM", GREEN_DARK, font_size=28).to_edge(UP, buff=0.8)

        self.play(FadeIn(header), run_time=0.3)
        self.play(GrowFromCenter(atom), run_time=min(1.0, t_appear * 0.6))
        self.safe_wait(t_appear * 0.4)

        # Duplicate collapses
        ghost = atom.copy().set_opacity(0.4).shift(RIGHT * 2)
        self.play(FadeIn(ghost, shift=LEFT), run_time=min(0.6, t_dedup * 0.4))
        self.play(ghost.animate.move_to(atom.get_center()), run_time=min(0.5, t_dedup * 0.3))
        self.remove(ghost)
        self.safe_wait(t_dedup * 0.3)

        # Red flash + shake
        red_flash = brutal_rect(2.5, 0.8, RED, fill_opacity=0.6).move_to(atom)
        self.play(FadeIn(red_flash), run_time=0.15)
        for dx in [LEFT * 0.1, RIGHT * 0.2, LEFT * 0.1]:
            self.play(
                atom.animate.shift(dx),
                red_flash.animate.shift(dx),
                run_time=0.05,
            )
        self.play(FadeOut(red_flash), run_time=0.2)
        self.safe_wait(t_reject - 0.5)

        # New atom
        atom2_block = brutal_rect(2.0, 0.8, GREEN)
        atom2_label = brutal_text("42", WHITE_BLOCK, font_size=36)
        atom2 = VGroup(atom2_block, atom2_label).next_to(atom, RIGHT, buff=1.5)
        self.play(GrowFromCenter(atom2), run_time=min(0.8, t_new * 0.6))
        self.safe_wait(t_hold)

        self.play(FadeOut(Group(*self.mobjects)), run_time=0.4)

    # ── Scene 3: Molecules ───────────────────────────────────────────

    def scene_03_molecules(self):
        dur = audio_duration("scene_03")
        bg = load_bg("scene_03")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_03_vo.mp3"))

        # Budget: setup(30%), rebind(35%), dim(15%), hold(20%)
        t_setup = dur * 0.30
        t_rebind = dur * 0.35
        t_dim = dur * 0.15
        t_hold = dur * 0.20

        header = brutal_text("MOLECULE", BLACK, font_size=28).to_edge(UP, buff=0.8)

        mol_block = brutal_rect(3, 0.8, WHITE_BLOCK)
        mol_label = brutal_text("name", BLACK, font_size=32)
        mol = VGroup(mol_block, mol_label).shift(UP * 1.0)

        atom_v1_block = brutal_rect(2.5, 0.8, GREEN)
        atom_v1_label = brutal_text('"Alice"', WHITE_BLOCK, font_size=30)
        atom_v1 = VGroup(atom_v1_block, atom_v1_label).shift(DOWN * 1.0)

        pointer = Line(mol.get_bottom(), atom_v1.get_top(), color=BLACK, stroke_width=3)
        arrow_tip = Triangle(color=BLACK, fill_color=BLACK, fill_opacity=1).scale(0.1)
        arrow_tip.next_to(pointer, DOWN, buff=0)

        self.play(FadeIn(header), run_time=0.3)
        self.play(FadeIn(mol), run_time=0.5)
        self.play(GrowFromCenter(atom_v1), run_time=0.5)
        self.play(Create(pointer), FadeIn(arrow_tip), run_time=0.4)
        self.safe_wait(t_setup - 1.7)

        # New atom + pointer rebind
        atom_v2_block = brutal_rect(2.5, 0.8, GREEN)
        atom_v2_label = brutal_text('"Bob"', WHITE_BLOCK, font_size=30)
        atom_v2 = VGroup(atom_v2_block, atom_v2_label).shift(DOWN * 1.0 + RIGHT * 3)

        self.play(GrowFromCenter(atom_v2), run_time=0.6)

        new_pointer = Line(mol.get_bottom(), atom_v2.get_top(), color=BLACK, stroke_width=3)
        new_tip = Triangle(color=BLACK, fill_color=BLACK, fill_opacity=1).scale(0.1)
        new_tip.next_to(new_pointer, DOWN, buff=0)

        self.play(
            Transform(pointer, new_pointer),
            Transform(arrow_tip, new_tip),
            run_time=0.6,
        )
        self.safe_wait(t_rebind - 1.2)

        # Old atom dims
        v1_tag = brutal_text("[v1]", BLACK, font_size=20).next_to(atom_v1, DOWN, buff=0.2)
        self.play(atom_v1.animate.set_opacity(0.35), FadeIn(v1_tag), run_time=0.6)
        self.safe_wait(t_dim - 0.6)

        self.safe_wait(t_hold)
        self.play(FadeOut(Group(*self.mobjects)), run_time=0.4)

    # ── Scene 4: Schemas ─────────────────────────────────────────────

    def scene_04_schemas(self):
        dur = audio_duration("scene_04")
        bg = load_bg("scene_04")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_04_vo.mp3"))

        # Budget: build(40%), shared(30%), pulse(10%), hold(20%)
        t_build = dur * 0.40
        t_shared = dur * 0.30
        t_pulse = dur * 0.10
        t_hold = dur * 0.20

        header = brutal_text("SCHEMA", BLUE, font_size=28).to_edge(UP, buff=0.8)
        schema_band = brutal_rect(6, 0.7, BLUE).shift(UP * 2.0)
        schema_label = brutal_text("UserProfile", WHITE_BLOCK, font_size=28).move_to(schema_band)

        fields = ["name", "email", "age"]
        mol_group = VGroup()
        atom_group = VGroup()
        lines = VGroup()

        for i, field in enumerate(fields):
            x = (i - 1) * 2.5
            mb = brutal_rect(2, 0.6, WHITE_BLOCK).move_to([x, 0.5, 0])
            ml = brutal_text(field, BLACK, font_size=24).move_to(mb)
            mol_group.add(VGroup(mb, ml))

            values = ['"Alice"', '"a@b.c"', "30"]
            ab = brutal_rect(2, 0.6, GREEN).move_to([x, -1.0, 0])
            al = brutal_text(values[i], WHITE_BLOCK, font_size=22).move_to(ab)
            atom_group.add(VGroup(ab, al))

            l1 = Line(schema_band.get_bottom() + [x * 0.3, 0, 0], mb.get_top(), color=BLACK, stroke_width=2)
            l2 = Line(mb.get_bottom(), ab.get_top(), color=BLACK, stroke_width=2)
            lines.add(l1, l2)

        anim_t = min(0.5, t_build / 6)
        self.play(FadeIn(header), run_time=0.3)
        self.play(FadeIn(schema_band, schema_label), run_time=anim_t)
        self.play(FadeIn(mol_group), run_time=anim_t)
        self.play(FadeIn(atom_group), run_time=anim_t)
        self.play(Create(lines), run_time=anim_t)
        self.safe_wait(t_build - 0.3 - 4 * anim_t)

        # Shared molecule
        schema2_band = brutal_rect(4, 0.6, BLUE).shift(UP * 2.0 + RIGHT * 4)
        schema2_label = brutal_text("ContactCard", WHITE_BLOCK, font_size=22).move_to(schema2_band)
        shared_line = DashedLine(
            schema2_band.get_bottom(),
            mol_group[0].get_right() + [0.2, 0, 0],
            color=MAGENTA, stroke_width=3,
        )
        shared_tag = brutal_text("shared", MAGENTA, font_size=18).next_to(shared_line, RIGHT, buff=0.2)

        self.play(FadeIn(schema2_band, schema2_label), run_time=0.5)
        self.play(Create(shared_line), FadeIn(shared_tag), run_time=0.5)
        self.safe_wait(t_shared - 1.0)

        # Read path pulse
        pulse_t = max(0.1, t_pulse / 6)
        for mol in mol_group:
            self.play(mol.animate.set_color(BLUE), run_time=pulse_t)
            self.play(mol.animate.set_color(BLACK), run_time=pulse_t)

        self.safe_wait(t_hold)
        self.play(FadeOut(Group(*self.mobjects)), run_time=0.4)

    # ── Scene 5: Access Control ──────────────────────────────────────

    def scene_05_access(self):
        dur = audio_duration("scene_05")
        bg = load_bg("scene_05")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_05_vo.mp3"))

        # Budget: build(30%), hold(30%), dim(15%), hold(25%)
        t_build = dur * 0.30
        t_hold1 = dur * 0.30
        t_dim = dur * 0.15
        t_hold2 = dur * 0.25

        header = brutal_text("ACCESS CONTROL", MAGENTA, font_size=28).to_edge(UP, buff=0.8)

        field_names = ["name", "email", "age"]
        viewer_names = ["Owner", "Friend", "Public"]
        matrix = [
            [True, True, True],
            [True, True, False],
            [True, False, False],
        ]

        grid = VGroup()
        cell_w, cell_h = 1.8, 0.7
        origin = np.array([-1.5, 0.5, 0])

        for j, viewer in enumerate(viewer_names):
            t = brutal_text(viewer, BLACK, font_size=18)
            t.move_to(origin + [(j + 1) * cell_w, cell_h, 0])
            grid.add(t)

        cell_rects = []
        for i, field in enumerate(field_names):
            t = brutal_text(field, BLACK, font_size=20)
            t.move_to(origin + [0, -i * cell_h, 0])
            grid.add(t)
            row_rects = []
            for j in range(3):
                allowed = matrix[i][j]
                color = ALLOW_GREEN if allowed else DENY_RED
                label = "ALLOW" if allowed else "DENY"
                rect = brutal_rect(cell_w - 0.1, cell_h - 0.1, color, fill_opacity=0.8)
                rect.move_to(origin + [(j + 1) * cell_w, -i * cell_h, 0])
                txt = brutal_text(label, WHITE_BLOCK, font_size=14).move_to(rect)
                cell = VGroup(rect, txt)
                grid.add(cell)
                row_rects.append(cell)
            cell_rects.append(row_rects)

        self.play(FadeIn(header), run_time=0.3)
        self.play(FadeIn(grid), run_time=min(1.0, t_build - 0.3))
        self.safe_wait(t_hold1)

        # Dim denied cells
        deny_cells = []
        for i in range(3):
            for j in range(3):
                if not matrix[i][j]:
                    deny_cells.append(cell_rects[i][j])

        self.play(
            *[cell.animate.set_opacity(0.2) for cell in deny_cells],
            run_time=min(0.8, t_dim),
        )
        self.safe_wait(t_hold2)
        self.play(FadeOut(Group(*self.mobjects)), run_time=0.4)

    # ── Scene 6: Full Picture + Close ────────────────────────────────

    def scene_06_full_picture(self):
        dur = audio_duration("scene_06")
        bg = load_bg("scene_06")
        self.add(bg)
        self.add_sound(str(ASSETS / "scene_06_vo.mp3"))

        # Budget: stack(30%), taglines(35%), logo(35%)
        t_stack = dur * 0.30
        t_tags = dur * 0.35
        t_logo = dur * 0.35

        header = brutal_text("THE FULL PICTURE", BLACK, font_size=28).to_edge(UP, buff=0.8)

        layers = [
            ("Schemas", BLUE),
            ("Molecules", WHITE_BLOCK),
            ("Atoms", GREEN),
            ("Permissions", MAGENTA),
        ]
        stack = VGroup()
        for i, (name, color) in enumerate(layers):
            band = brutal_rect(7, 0.7, color)
            label = brutal_text(name, BLACK if color == WHITE_BLOCK else WHITE_BLOCK, font_size=24)
            label.move_to(band)
            layer = VGroup(band, label)
            layer.shift(DOWN * (i * 0.9) + UP * 1.0)
            stack.add(layer)

        layer_t = min(0.4, (t_stack - 0.3) / 4)
        self.play(FadeIn(header), run_time=0.3)
        for layer in stack:
            self.play(FadeIn(layer, shift=RIGHT * 0.3), run_time=layer_t)
        self.safe_wait(t_stack - 0.3 - 4 * layer_t)

        # Taglines
        tags = ["Semantic storage.", "Controlled disclosure.", "Data sovereignty."]
        tag_t = min(0.5, t_tags / (len(tags) * 2))
        for i, tag in enumerate(tags):
            t = brutal_text(tag, BLACK, font_size=32)
            t.shift(DOWN * 2.2 + DOWN * i * 0.5)
            self.play(FadeIn(t, shift=UP * 0.2), run_time=tag_t)
            self.safe_wait(tag_t)

        # Fade to logo
        self.play(FadeOut(Group(*self.mobjects)), run_time=0.5)

        logo_block = brutal_rect(5, 1.2, BLACK)
        logo_text = brutal_text("FOLDDB", WHITE_BLOCK, font_size=72)
        logo = VGroup(logo_block, logo_text)
        self.play(FadeIn(logo, scale=0.9), run_time=0.8)
        self.safe_wait(t_logo - 1.8)
        self.play(FadeOut(logo), run_time=0.5)
