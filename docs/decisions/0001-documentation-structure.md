# 0001: Documentation follows Diataxis

- **Status:** accepted (structure) · proposed (generator and design)
- **Date:** 2026-11-27

## Context

Documentation grew as a few long pages ([GUIDE.md](../GUIDE.md), 1266 lines;
[protocol-spec.md](../protocol-spec.md), 1758 lines) that mix tutorial, how-to,
reference and explanation. There was no entry point, no component map, no
decision log, no threat model, and version labels had drifted (README and GUIDE
said v0.5 while releases were at v0.9).

## Options

1. Keep the long pages and only add to them.
2. Rewrite everything into a new structure at once.
3. Add a Diataxis structure around the existing pages, link to them, and move
   content one task at a time.

## Decision

Option 3. Four doors (tutorials, how-to guides, reference, explanation) plus
architecture, security, decisions and verification, with [docs/index.md](../index.md)
as the entry point. The rules for writing and reviewing pages are in the
`technical-docs` skill. Generated reference (the [settings reference](../reference/settings.md))
comes from the code and is checked by `tools/docs_check.py`.

**Not decided:** which site generator to use (mdBook, MkDocs Material or
Docusaurus). Recommendation when the time comes: **MkDocs Material**, because
it gives a sidebar, search, admonitions, Mermaid and light/dark themes from plain
Markdown with one config file, matching the calm, text-first look of the Divio
documentation. Until then the pages are plain Markdown that GitHub renders.

## Consequences

- Existing pages keep working; links into them stay valid.
- A page must say its type and status. Mixed pages are labelled as mixed.
- Documentation changes ship in the same change as the behavior
  ([AGENTS.md](../../AGENTS.md)).

## Evidence

Gap analysis of 2026-11-27 against the `technical-docs` skill; sources: the
Mintlify guide, the Divio documentation system, the ProductFlo article.
