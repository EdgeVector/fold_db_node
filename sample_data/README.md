# Sample Data for Smart Folder Ingestion Testing

This folder simulates a real user's Documents folder with a mix of personal data, media, config files, saved webpages, and binaries. It's designed to test the LLM-powered smart folder scanner's ability to classify files using directory context.

## Quick Start

### 1. Set your Anthropic API key

An [Anthropic](https://console.anthropic.com/settings/keys) API key is the only external requirement. Set it before starting the server:

```bash
export ANTHROPIC_API_KEY=your_key_here
```

### 2. Start the server

From the `fold_db_node/` directory:

```bash
# Recommended: local storage + production schema service
./run.sh --local

# Fully offline (no internet required after build):
./run.sh --local --local-schema
```

The `--local-schema` flag starts a local schema service on port **9102** (built from the sibling `schema_service` checkout — see fold_db_node `CLAUDE.md` "Schema service" for details).

> **Ports auto-slot.** When multiple agents run in parallel, `run.sh` walks the backend port forward in 9101..=9199, the schema port forward from 9102, and the Vite port forward in 5173..=5299. The canonical reference for what your running instance picked is `~/.folddb-slots/<backend_port>.json` — fields: `port` (backend HTTP), `schema_port` (schema service), `vite_port` (frontend). The examples below use the dev defaults (9101 / 9102 / 5173). Substitute your slot-file values if `run.sh` reported different numbers.

### 3. Scan the sample data

Open the frontend (default http://localhost:5173 — use the `vite_port` from `~/.folddb-slots/<backend_port>.json` if it slotted elsewhere, e.g. 5179 or the first free port in 5173..=5299), go to the Smart Folder tab, and click **"Try sample data"** to auto-fill the path, then click **Scan**.

> **Dev-only button.** The **"Try sample data"** shortcut is gated by `import.meta.env.DEV` (see `src/server/static-react/src/components/tabs/smart-folder/FolderInput.tsx`) and only appears in `npm run dev` / `./run.sh` builds. It is intentionally NOT bundled into the production Tauri release per the repo policy against shipping sample/fixture data to prod. In a release build, type the folder path manually.

Or via API:
```bash
# Substitute the backend port from ~/.folddb-slots/<port>.json if run.sh slotted elsewhere
BACKEND_PORT=9101
UH=test_user
curl -X POST http://localhost:$BACKEND_PORT/api/ingestion/smart-folder/scan \
  -H "Content-Type: application/json" \
  -H "X-User-Hash: $UH" \
  -d '{"folder_path": "sample_data", "max_files": 100}'
```

### 4. Ingest

Review the scan results and click **Proceed** to ingest the recommended files.

### 5. Query an ingested schema

After ingestion completes, query the data via `/api/query`. The example below reads back the `Journal Entries` schema created by ingesting `sample_data/journal/*.txt`:

```bash
curl -X POST http://localhost:$BACKEND_PORT/api/query \
  -H "Content-Type: application/json" -H "X-User-Hash: $UH" \
  -d '{"schema_name":"Journal Entries","fields":["title","body"]}'
```

> **The LLM picks the schema name.** The classifier may name the resulting schema differently from the source folder — e.g. ingesting `taxes_2024/` has produced a `W2 Tax Forms` schema rather than `Taxes 2024`. To discover the actual names after ingestion, list them:
>
> ```bash
> curl -s http://localhost:$BACKEND_PORT/api/schemas -H "X-User-Hash: $UH"
> ```
>
> Then pass whichever `schema_name` you find into `/api/query`.

## Directory Structure

```
sample_data/
├── blog_posts.json              # Personal blog content
├── meeting_notes.txt            # Work meeting notes
├── products.csv                 # Product catalog
├── users.json                   # User records
├── contacts/
│   └── address_book.json        # Personal contacts
├── config/
│   ├── .bashrc                  # Shell config (skipped — dotfile)
│   ├── settings.json            # Editor settings (currently NOT skipped — see "What to expect")
│   ├── old_backup.exe           # Binary (skipped — non-ingestible extension)
│   └── helper_tool.dll          # Binary (skipped — non-ingestible extension)
├── finance/
│   ├── bank_statement_jan2025.csv  # Bank transactions
│   ├── investments.json            # Portfolio holdings
│   └── tax_receipt_2024.pdf        # PDF with tax receipt text
├── health/
│   ├── doctor_visits.txt        # Medical visit notes
│   └── medications.json         # Prescription records
├── insurance/
│   ├── auto_policy.json         # Car insurance details
│   └── declarations_page.pdf    # PDF with insurance declarations
├── journal/
│   ├── 2025-01-15.txt           # Daily journal entry
│   └── 2025-01-20.txt           # Daily journal entry
├── photos/
│   ├── profile_pic.png          # 64x64 PNG image
│   ├── animals/                 # Animal photos (golden retriever, tabby cat, etc.)
│   ├── diagrams/                # SVG diagrams (architecture, ER diagram, flowchart)
│   ├── family/
│   │   ├── christmas_2024.jpg   # 64x64 JPEG image
│   │   └── thanksgiving_2024.jpg
│   ├── landscapes/              # Nature landscapes (mountain, ocean, desert, etc.)
│   ├── paintings/               # Famous paintings (Mona Lisa, Starry Night, etc.)
│   ├── profile/                 # Portrait photos (studio, outdoor, creative)
│   ├── screenshots/             # SVG screenshots (terminal, dashboard)
│   └── vacation_2024/
│       ├── IMG_4521.jpg         # 64x64 JPEG images
│       ├── IMG_4522.jpg
│       └── IMG_4523.jpg
├── recipes/
│   ├── grandmas_cookies.txt     # Family recipe
│   └── meal_plan.csv            # Weekly meal plan
├── saved_webpages/
│   └── bank_of_america/         # "Save as complete webpage"
│       ├── account_summary.html # The actual content
│       ├── css/
│       │   ├── styles.css       # Scaffolding (should skip)
│       │   └── icons.woff2      # Font file (should skip)
│       └── images/
│           ├── ajax-loader.gif  # Scaffolding (should skip)
│           ├── boa_logo.gif     # Scaffolding (should skip)
│           └── spacer.gif       # Scaffolding (should skip)
├── school/
│   ├── cs101/
│   │   ├── homework3.txt        # Graded homework
│   │   └── syllabus.pdf         # PDF with course syllabus
│   └── math201/
│       └── notes_linear_algebra.md  # Course notes
├── taxes_2024/
│   ├── w2_summary.json          # W-2 tax data
│   └── charitable_donations.csv # Donation records
├── travel/
│   ├── packing_list.txt         # Trip planning
│   ├── flights/
│   │   └── sfo_to_tokyo_2025.json  # Flight booking
│   └── hotels/
│       └── tokyo_hotel.json     # Hotel reservation
├── coding_projects/                 # Coding projects (should be auto-skipped)
│   ├── my_website/              # Node.js project (has package.json)
│   │   ├── package.json
│   │   ├── index.js
│   │   └── README.md
│   ├── rust_cli/                # Rust project (has Cargo.toml)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       └── main.rs
│   └── data_analysis/           # Python project (has pyproject.toml)
│       ├── pyproject.toml
│       ├── analysis.py
│       └── requirements.txt
└── work/
    ├── expenses/
    │   └── jan_2025_expenses.csv # Expense report
    ├── presentations/
    │   └── team_retro_q4.md     # Team retrospective
    └── project_notes/
        └── q1_goals.json        # Quarterly goals
```

## What to expect

The LLM classifier should:
- **Recommend** personal data: finance, health, contacts, journal, travel bookings, taxes, insurance, recipes
- **Skip** dotfiles (e.g. `.bashrc` — filtered by the scanner because it starts with `.`), binaries (`.exe`, `.dll`), font files (`.woff2`)
- **Skip** saved webpage scaffolding (CSS, GIFs inside `bank_of_america/`) while possibly recommending the HTML content
- **Recommend** photos and PDFs as media/personal data (these are valid files and will be processed via the vision model)
- **Auto-skip** coding projects (`coding_projects/`) — directories whose immediate children contain `package.json`, `Cargo.toml`, or `pyproject.toml` are skipped entirely (the directory and all descendants) before LLM classification. See `src/ingestion/smart_folder/scanner.rs::is_coding_project_root`.

Aspirational behavior — not yet implemented:
- **Skip** by-name config files like `settings.json` — currently NOT skipped (the heuristic treats `.json` as text/data). Only dotfiles and non-ingestible extensions are dropped today; LLM judgment may still override.

## Dependencies

All dependencies are wired up by `run.sh`:
- **Rust backend** — built by `run.sh`
- **React frontend** — `npm install` handled by `run.sh`
- **Local schema service** — built from the sibling `schema_service` checkout when you pass `--local-schema`; orchestrated automatically (see fold_db_node `CLAUDE.md` "Schema service")
- **Sample files** — all images are valid 64x64 JPEG/PNG, all PDFs contain readable text

The only external requirement is an **Anthropic API key** for AI-powered classification and ingestion. Without `ANTHROPIC_API_KEY` set in the shell that runs `./run.sh`, scan + ingest will fail; everything else (build, query, the dev UI) still works.
