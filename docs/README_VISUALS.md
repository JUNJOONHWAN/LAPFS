# README visual update

## Before-impact report

Request: improve the public LAPFS README with visual architecture diagrams, HTML and pictures.
Scope: README, two static SVG diagrams, an updated SVG title graphic, a self-contained interactive HTML guide, PNG exports, and this evidence document. Source: beta.7 transaction implementation and docs/validation/native-cow-beta7.json. No writer, mount, queue, runtime, scheduler, existing device or license changes.

Modeling job: explanatory system architecture and checkpoint state transitions for users moving an APFS device between Linux and Mac. Primary route: annotated swimlane and state diagrams in SVG; fallback: static SVG plus text tables. HTML owns only presentation and step selection, with no filesystem operations or external calls.

Renderer: native SVG and HTML/CSS/JavaScript, one guide instance, four selectable steps. No dependencies, remote fonts, tracking or automatic animation. URL query `step=0..3` stores selection; invalid values fall back to0. No persistent user data. Mobile uses a single-column sequence, desktop two lanes. Neutral ink/paper, teal for new blocks, blue for active checkpoint, amber for pending state; all states also have text labels. Keyboard buttons, visible focus, aria-live description and static text alternatives are required.

QA: render desktop and390px portrait, inspect screenshots, check overflow and SVG text bounds, select all four states with keyboard/click, reload URL state and verify evidence counts against the source receipt. Public beta.7 binary archives remain immutable; publish a separate documentation bundle and update main README/release links.

## After-impact report

Updated README hierarchy: current beta.7 evidence first, two diagrams, step-by-step HTML link, readable operation table, historical device evidence collapsed separately. Corrected contradictions for chmod, atime/mtime, symlink create/remove and empty-directory rmdir. Added three SVG sources and three rendered PNG exports, a dependency-free HTML guide and JSON QA receipt.

Visual inspection: 1440px desktop and390px portrait screenshots reviewed. SVG title/architecture/checkpoint exports reviewed. No SVG text falls outside its viewBox; no mobile horizontal overflow. All four steps, keyboard Enter activation, URL reload/invalid-state handling and no-JavaScript text fallback passed; browser page errors0. Static SVG plus textual bullets/tables remain available in the GitHub README.

Evidence values are unchanged:430 native interruption cases,296 Rust tests,3 Mac/Linux cycles,272,629,778-byte multi-chunk file. Diagrams explicitly distinguish conceptual block counts, design assumptions, external queue durability and unqualified hardware power cuts.

Changed artifacts only: README.md, docs/architecture.html, docs/assets/lapfs-{banner,architecture,checkpoint}.{svg,png}, docs/validation/readme-visuals.json and this report. Published writer SHA remains dccc7cead3774b9b1d247c3ac2e1334fdfa114e44e70dab087f158da52fb3b32; no remount, device writes, scheduler or research data changes. Existing beta.7 code archives are retained and a separate visual documentation bundle is added.

## Manual terminology revision

User correction: noun-first manual labels; no conversational headings or promotional sentences. README converted to specifications, tables and operation lists. SVG/HTML titles changed to storage location, interruption states, write stages and validation scope. All four interactive state descriptions converted to labeled terms. Desktop/mobile interaction, SVG bounds and static fallback checks rerun. PNG exports regenerated from the revised SVGs.

Transfer-option observation: Mac openrsync protocol29 / rsync2.6.9 compatibility; DGX rsync3.2.7. Mac `--progress --version` accepted. README now identifies `--progress` as the portable progress option and `--info=progress2` as version-dependent; no transfer or filesystem changes.
